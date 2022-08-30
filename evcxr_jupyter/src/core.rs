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

// Note, to avoid potential deadlocks, e