// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

// We currently use the json crate. Could probably rewrite to use serde-json. At
// the time this was originally written we couldn't due to
// https://github.com/rust-lang/rust/issues/45601 - but that's now long fixed
// and we've dropped support for old version for rustc prior to the fix.

use anyhow::anyhow;
use anyhow::Result;
use std::fs;

#[derive(Debug, Clone)]
pub(crate) struct Control {
    pub(crate) control_port: u16,
    pub(crate) shell_port: u16,
    pub(crate) stdin_port: u16,
    pub(crate) hb_port: u16,
    pub(crate) iopub_port: u16,
    pub(crate) transport: String,
    pub(crate) ip: String,
    pub(crate) key: String,
}

macro_rules! parse_to_var {
    ($control_json:expr, $name:ide