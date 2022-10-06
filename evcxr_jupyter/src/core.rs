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
                .await?;
        }
    }

    async fn handle_execution_requests(
        self,
        context: &Arc<std::sync::Mutex<CommandContext>>,
        receiver: &mut tokio::sync::mpsc::UnboundedReceiver<JupyterMessage>,
        execution_reply_sender: &tokio::sync::mpsc::UnboundedSender<JupyterMessage>,
    ) -> Result<()> {
        let mut execution_count = 1;
        loop {
            let message = match receiver.recv().await {
                Some(x) => x,
                None => {
                    // Other end has closed. This is expected when we're shuting
                    // down.
                    return Ok(());
                }
            };

            // If we want this clone to be cheaper, we probably only need the header, not the
            // whole message.
            *self.latest_execution_request.lock().await = Some(message.clone());
            let src = message.code().to_owned();
            execution_count += 1;
            message
                .new_message("execute_input")
                .with_content(object! {
                    "execution_count" => execution_count,
                    "code" => src
                })
                .send(&mut *self.iopub.lock().await)
                .await?;

            let context = Arc::clone(context);
            let server = self.clone();
            let (eval_result, message) = tokio::task::spawn_blocking(move || {
                let eval_result = context.lock().unwrap().execute_with_callbacks(
                    message.code(),
                    &mut evcxr::EvalCallbacks {
                        input_reader: &|input_request| {
                            server.tokio_handle.block_on(async {
                                server
                                    .request_input(
                                        &message,
                                        &input_request.prompt,
                                        input_request.is_password,
                                    )
                                    .await
                                    .unwrap_or_default()
                            })
                        },
                    },
                );
                (eval_result, message)
            })
            .await?;
            match eval_result {
                Ok(output) => {
                    if !output.is_empty() {
                        // Increase the odds that stdout will have been finished being sent. A
                        // less hacky alternative would be to add a print statement, then block
                        // waiting for it.
                        tokio::time::sleep(Duration::from_millis(1)).await;
                        let mut data = HashMap::new();
                        // At the time of writing the json crate appears to have a generic From
                        // implementation for a Vec<T> where T implements Into<JsonValue>. It also
                        // has conversion from HashMap<String, JsonValue>, but it doesn't have
                        // conversion from HashMap<String, T>. Perhaps send a PR? For now, we
                        // convert the values manually.
                        for (k, v) in output.content_by_mime_type {
                            if k.contains("json") {
                                data.insert(k, json::parse(&v).unwrap_or_else(|_| json::from(v)));
                            } else {
                                data.insert(k, json::from(v));
                            }
                        }
                        message
                            .new_message("execute_result")
                            .with_content(object! {
                                "execution_count" => execution_count,
                                "data" => data,
                                "metadata" => object!(),
                            })
                            .send(&mut *self.iopub.lock().await)
                            .await?;
                    }
                    if let Some(duration) = output.timing {
                        // TODO replace by duration.as_millis() when stable
                        let ms = duration.as_secs() * 1000 + u64::from(duration.subsec_millis());
                        let mut data: HashMap<String, JsonValue> = HashMap::new();
                        data.insert(
                            "text/html".into(),
                            json::from(format!(
                                "<span style=\"color: rgba(0,0,0,0.4);\">Took {}ms</span>",
                                ms
                            )),
                        );
                        message
                            .new_message("execute_result")
                            .with_content(object! {
                                "execution_count" => execution_count,
                                "data" => data,
                                "metadata" => object!(),
                            })
                            .send(&mut *self.iopub.lock().await)
                            .await?;
                    }
                    execution_reply_sender.send(message.new_reply().with_content(object! {
                        "status" => "ok",
                        "execution_count" => execution_count,
                    }))?;
                }
                Err(errors) => {
                    self.emit_errors(&errors, &message, message.code(), execution_count)
                        .await?;
                    execution_reply_sender.send(message.new_reply().with_content(object! {
                        "status" => "error",
                        "execution_count" => execution_count
                    }))?;
                }
            };
        }
    }

    async fn request_input(
        &self,
        current_request: &JupyterMessage,
        prompt: &str,
        password: bool,
    ) -> Option<String> {
        if current_request.get_content()["allow_stdin"].as_bool() != Some(true) {
            return None;
        }
        let mut stdin = self.stdin.lock().await;
        let stdin_request = current_request
            .new_reply()
            .with_message_type("input_request")
            .with_content(object! {
                "prompt" => prompt,
                "password" => password,
            });
        stdin_request.send(&mut *stdin).await.ok()?;

        let input_response = JupyterMessage::read(&mut *stdin).await.ok()?;
        input_response.get_content()["value"]
            .as_str()
            .map(|value| value.to_owned())
    }

    async fn handle_shell<S: zeromq::SocketRecv + zeromq::SocketSend>(
        self,
        mut connection: Connection<S>,
        execution_channel: &tokio::sync::mpsc::UnboundedSender<JupyterMessage>,
        execution_reply_receiver: &mut tokio::sync::mpsc::UnboundedReceiver<JupyterMessage>,
        context: Arc<std::sync::Mutex<CommandContext>>,
    ) -> Result<()> {
        loop {
            let message = JupyterMessage::read(&mut connection).await?;
            self.handle_shell_message(
                message,
                &mut connection,
                execution_channel,
                execution_reply_receiver,
                &context,
            )
            .await?;
        }
    }

    async fn handle_shell_message<S: zeromq::SocketRecv + zeromq::SocketSend>(
        &self,
        message: JupyterMessage,
        connection: &mut Connection<S>,
        execution_channel: &tokio::sync::mpsc::UnboundedSender<JupyterMessage>,
        execution_reply_receiver: &mut tokio::sync::mpsc::UnboundedReceiver<JupyterMessage>,
        context: &Arc<std::sync::Mutex<CommandContext>>,
    ) -> Result<()> {
        // Processing of every message should be enclosed between "busy" and "idle"
        // see https://jupyter-client.readthedocs.io/en/latest/messaging.html#messages-on-the-shell-router-dealer-channel
        // Jupiter Lab doesn't use the kernel until it received "idle" for kernel_info_request
        message
            .new_message("status")
            .with_content(object! {"execution_state" => "busy"})
            .send(&mut *self.iopub.lock().await)
            .await?;
        let idle = message
 