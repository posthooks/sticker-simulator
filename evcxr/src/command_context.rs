// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use crate::code_block::CodeBlock;
use crate::code_block::CodeKind;
use crate::code_block::CommandCall;
use crate::code_block::Segment;
use crate::code_block::{self};
use crate::crash_guard::CrashGuard;
use crate::errors::bail;
use crate::errors::CompilationError;
use crate::errors::Error;
use crate::errors::Span;
use crate::errors::SpannedMessage;
use crate::eval_context::ContextState;
use crate::eval_context::EvalCallbacks;
use crate::rust_analyzer::Completion;
use crate::rust_analyzer::Completions;
use crate::EvalContext;
use crate::EvalContextOutputs;
use crate::EvalOutputs;
use anyhow::Result;
use once_cell::sync::OnceCell;

/// A higher level interface to EvalContext. A bit closer to a Repl. Provides commands (start with
/// ':') that alter context state or print information.
pub struct CommandContext {
    print_timings: bool,
    eval_context: EvalContext,
    last_errors: Vec<CompilationError>,
}

impl CommandContext {
    pub fn new() -> Result<(CommandContext, EvalContextOutputs), Error> {
        let (eval_context, eval_context_outputs) = EvalContext: