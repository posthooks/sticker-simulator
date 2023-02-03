
// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use super::scan::validate_source_fragment;
use super::scan::FragmentValidity;
use crate::bginit::BgInitMutex;
use colored::*;
use evcxr::CommandContext;
use evcxr::Completions;
use evcxr::Error;
use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::ValidationContext;
use rustyline::validate::ValidationResult;
use rustyline::validate::Validator;
use rustyline::Context;
use rustyline::Helper;
use std::borrow::Cow;
use std::sync::Arc;

pub struct EvcxrRustylineHelper {
    command_context: Arc<BgInitMutex<Result<CommandContext, Error>>>,
}

impl EvcxrRustylineHelper {
    pub fn new(command_context: Arc<BgInitMutex<Result<CommandContext, Error>>>) -> Self {
        Self { command_context }
    }
}

// Have to implement a bunch of traits as mostly noop...

impl Hinter for EvcxrRustylineHelper {
    type Hint = String;
}

impl Completer for EvcxrRustylineHelper {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<String>)> {
        let completions = match &mut *self.command_context.lock() {
            Ok(command_context) => command_context
                .completions(line, pos)
                .unwrap_or_else(|_| Completions::default()),
            Err(e) => {
                return Err(rustyline::error::ReadlineError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.clone(),
                )))
            }
        };
        let res: Vec<String> = completions
            .completions
            .into_iter()
            .map(|c| c.code)
            .collect();
        Ok((completions.start_offset, res))
    }
}