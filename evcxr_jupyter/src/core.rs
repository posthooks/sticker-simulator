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
            // had no subscriptions. The vscode extensi