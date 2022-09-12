// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::connection::Connection;
use crate::control_file;
use crate::jupyter_message::JupyterMessage;
use anyhow::bail;
use anyhow::Result;
use ariadne::sources;
use colored::*;
use crossbeam_channel::Select;
use evcxr::CommandContext;
use evcxr::Theme;
use json::JsonValue;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

// Note, to avoid potential deadlocks, each thread should lock at most one mutex at a time.
#[derive(Clone)]
pub(crate) struct Server {
    iopub: Arc<Mutex<Connection<zeromq::PubSocket>>>,
    stdin: Arc<Mutex<Connection<zeromq::RouterSocket>>>,
    latest_execution_request: Arc<Mutex<Option<JupyterMessage>>>,
    shutdown_sender: Arc<Mutex<Option<crossbeam_channel::Sender<()>>>>,
    tokio_handle: tokio::runtime::Handle,
}

struct ShutdownReceiver {
    // Note, this needs to be a crossbeam channel because
    // start_output_pass_through_thread selects on this and other crossbeam
    // channels.
    recv: crossbeam_channel::Receiver<()>,
}

impl Server {
    pub(crate) fn run(config: &control_file::Control) -> Result<()> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            // We only technically need 1 thread. However we've observed that
            // when using vscode's jupyter extension, we can get requests on the
            // shell socket before we have any subscribers on iopub. The iopub
            // subscription then completes, but the execution_state="idle"
            // message(s) have already been sent to a channel that at the time
            // had no subscriptions. The vscode extension then waits
            // indefinitely for an execution_state="idle" message that will
            // never come. Having multiple threads at least reduces the chances
            // of this happening.
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();
        let handle = runtime.handle().clone();
        runtime.block_on(async {
            let shutdown_receiver = Self::start(config, handle).await?;
            shutdown_receiver.wait_for_shutdown().await;
            let result: Result<()> = Ok(());
            result
        })?;
        Ok(())
    }

    async fn start(
        config: &control_file::Control,
        tokio_handle: tokio::runtime::Handle,
    ) -> Result<ShutdownReceiver> {
        let mut heartbeat = bind_socket::<zeromq::RepSocket>(config, config.hb_port).await?;
        let shell_socket = bind_socket::<zeromq::RouterSocket>(config, config.shell_port).await?;
        let control_socket =
            bind_socket::<zeromq::RouterSocket>(config, config.control_port).await?;
        let stdin_socket = bind_socket::<zeromq::RouterSocket>(config, config.stdin_port).await?;
        let iopub_socket = bind_socket::<zeromq::PubSocket>(config, config.iopub_port).await?;
        let iopub = Arc::new(Mutex::new(iopub_socket));

        let (shutdown_sender, shutdown_receiver) = crossbeam_channel::unbounded();

        let server = Server {
            iopub,
            latest_execution_request: Arc::new(Mutex::new(None)),
            stdin: Arc::new(Mutex::new(stdin_socket)),
            shutdown_sender: Arc::new(Mutex::new(Some(shutdown_sender))),
            tokio_handle,
        };

        let (execution_sender, mut execution_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (execution_response_sender, mut execution_response_receiver) =
            tokio::sync::mpsc::unbounded_channel();

        tokio::spawn(async move {
            if let Err(error) = Self::handle_hb(&mut heartbeat).await {
                eprintln!("hb error: {error:?}");
            }
        });
        let (mut context, outputs) = CommandContext::new()?;
        context.execute(":load_config")?;
        let process_handle = context.process_handle();
        let context = Arc::new(std::sync::Mutex::new(context));
        {
            let server = server.clone();
            tokio::spawn(async move {
                if let Err(error) = server.handle_control(control_socket, process_handle).await {
                    eprintln!("control error: {error:?}");
                }
            });
        }
        {
            let context = context.clone();
            let server = server.clone();
            tokio::spawn(async move {
                let result = server
                    .handle_shell(
                        shell_socket,
                        &execution_sender,
                        &mut execution_response_receiver,
                        context,
                    )
                    .await;
                if let Err(error) = result {
                    eprintln!("shell error: {error:?}");
                }
            });
        }
        {
            let server = server.clone();
            tokio::spawn(async move {
                let result = server
                    .handle_execution_requests(
                        &context,
                        &mut execution_receiver,
                        &execution_response_sender,
                    )
                    .await;
                if let Err(error) = result {
                    eprintln!("execution error: {error:?}");
                }
            });
        }
        server
            .clone()
            .start_output_pass_through_thread(
                vec![("stdout", outputs.stdout), ("stderr", outputs.stderr)],
                shutdown_receiver.clone(),
            )
            .await;
        Ok(ShutdownReceiver {
            recv: shutdown_receiver,
        })
    }

    async fn signal_shutdown(&mut self) {
        self.shutdown_sender.lock().await.take();
    }

    async fn handle_hb(connection: &mut Connection<zeromq::RepSocket>) -> Result<()> {
        use zeromq::SocketRecv;
        use zeromq::SocketSend;
        loop {
            connection.socket.recv().await?;
            connection
                .socket
                .send(zeromq::ZmqMessage::from(b"ping".to_vec()))
                .awa