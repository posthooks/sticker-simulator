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
        let (eval_context, eval_context_outputs) = EvalContext::new()?;
        let command_context = CommandContext::with_eval_context(eval_context);
        Ok((command_context, eval_context_outputs))
    }

    pub fn with_eval_context(eval_context: EvalContext) -> CommandContext {
        CommandContext {
            print_timings: false,
            eval_context,
            last_errors: Vec::new(),
        }
    }

    #[doc(hidden)]
    pub fn new_for_testing() -> (CommandContext, EvalContextOutputs) {
        let (eval_context, outputs) = EvalContext::new_for_testing();
        (Self::with_eval_context(eval_context), outputs)
    }

    pub fn execute(&mut self, to_run: &str) -> Result<EvalOutputs, Error> {
        self.execute_with_callbacks(to_run, &mut EvalCallbacks::default())
    }

    pub fn check(&mut self, code: &str) -> Result<Vec<CompilationError>, Error> {
        let (user_code, code_info) = CodeBlock::from_original_user_code(code);
        let (non_command_code, state, errors) = self.prepare_for_analysis(user_code)?;
        if !errors.is_empty() {
            // If we've got errors while preparing, probably due to bad :dep commands, then there's
            // no point running cargo check as it'd just give us additional follow-on errors which
            // would be confusing.
            return Ok(errors);
        }
        self.eval_context.check(non_command_code, state, &code_info)
    }

    pub fn process_handle(&self) -> Arc<Mutex<std::process::Child>> {
        self.eval_context.process_handle()
    }

    pub fn variables_and_types(&self) -> impl Iterator<Item = (&str, &str)> {
        self.eval_context.variables_and_types()
    }

    pub fn reset_config(&mut self) {
        self.eval_context.reset_config();
    }

    pub fn defined_item_names(&self) -> impl Iterator<Item = &str> {
        self.eval_context.defined_item_names()
    }

    pub fn execute_with_callbacks(
        &mut self,
        to_run: &str,
        callbacks: &mut EvalCallbacks,
    ) -> Result<EvalOutputs, Error> {
        let mut state = self.eval_context.state();
        state.clear_non_debug_relevant_fields();
        let mut guard = CrashGuard::new(|| {
            eprintln!(
                r#"
=============================================================================
Panic detected. Here's some useful information if you're filing a bug report.
<CODE>
{to_run}
</CODE>
<STATE>
{state:?}
</STATE>"#
            );
        });
        let result = self.execute_with_callbacks_internal(to_run, callbacks);
        guard.disarm();
        result
    }

    fn execute_with_callbacks_internal(
        &mut self,
        to_run: &str,
        callbacks: &mut EvalCallbacks,
    ) -> Result<EvalOutputs, Error> {
        use std::time::Instant;
        let mut eval_outputs = EvalOutputs::new();
        let start = Instant::now();
        let mut state = self.eval_context.state();
        let mut non_command_code = CodeBlock::new();
        let (user_code, code_info) = CodeBlock::from_original_user_code(to_run);
        for segment in user_code.segments {
            if let CodeKind::Command(command) = &segment.kind {
                eval_outputs.merge(self.execute_command(
                    command,
                    &segment,
                    &mut state,
                    &command.args,
                )?);
            } else {
                non_command_code = non_command_code.with_segment(segment);
            }
        }
        let result =
            self.eval_context
                .eval_with_callbacks(non_command_code, state, &code_info, callbacks);
        let duration = start.elapsed();
        match result {
            Ok(m) => {
                eval_outputs.merge(m);
                if self.print_timings {
                    eval_outputs.timing = Some(duration);
                }
                Ok(eval_outputs)
            }
            Err(Error::CompilationErrors(errors)) => {
                self.last_errors = errors.clone();
                Err(Error::CompilationErrors(errors))
            }
            x => x,
        }
    }

    pub fn set_opt_level(&mut self, level: &str) -> Result<(), Error> {
        self.eval_context.set_opt_level(level)
    }

    pub fn last_source(&self) -> std::io::Result<String> {
        self.eval_context.last_source()
    }

    /// Returns completions within `src` at `position`, which should be a byte offset. Note, this
    /// function requires &mut self because it mutates internal state in order to determine
    /// completions. It also assumes exclusive access to those resources. However there should be
    /// any visible side effects.
    pub fn completions(&mut self, src: &str, position: usize) -> Result<Completions> {
        let (user_code, code_info) = CodeBlock::from_original_user_code(src);
        if let Some((segment, offset)) = user_code.command_containing_user_offset(position) {
            return self.command_completions(segment, offset, position);
        }
        let (non_command_code, state, _errors) = self.prepare_for_analysis(user_code)?;
        self.eval_context
            .completions(non_command_code, state, &code_info.nodes, position)
    }

    fn prepare_for_analysis(
        &mut self,
        user_code: CodeBlock,
    ) -> Result<(CodeBlock, ContextState, Vec<CompilationError>)> {
        let mut non_command_code = CodeBlock::new();
        let mut state = self.eval_context.state();
        let mut errors = Vec::new();
        for segment in user_code.segments {
            if let CodeKind::Command(command) = &segment.kind {
                if let Err(error) =
                    self.process_command(command, &segment, &mut state, &command.args, true)
                {
                    errors.push(error);
                }
            } else {
                non_command_code = non_command_code.with_segment(segment);
            }
        }
        self.eval_context.write_cargo_toml(&state)?;
        Ok((non_command_code, state, errors))
    }

    fn command_completions(
        &self,
        segment: &Segment,
        offset: usize,
        full_position: usize,
    ) -> Result<Completions> {
        let existing = &segment.code[0..offset];
        let mut completions = Completions {
            start_offset: full_position - offset,
            end_offset: full_position,
            ..Completions::default()
        };
        for cmd in Self::commands_by_name().keys() {
            if cmd.starts_with(existing) {
                completions.completions.push(Completion {
                    code: (*cmd).to_owned(),
                })
            }
        }
        Ok(completions)
    }

    fn load_config(&mut self, quiet: bool) -> Result<EvalOutputs, Error> {
        let mut outputs = EvalOutputs::new();
        if let Some(config_dir) = crate::config_dir() {
            let config_file = config_dir.join("init.evcxr");
            if config_file.exists() {
                if !quiet {
                    println!("Loading startup commands from {config_file:?}");
                }
                let contents = std::fs::read_to_string(config_file)?;
                for line in contents.lines() {
                    outputs.merge(self.execute(line)?);
                }
            }
            // Note: Loaded *after* init.evcxr so that it can access `:dep`s (or
            // any other state changed by :commands) specified in the init file.
            let prelude_file = config_dir.join("prelude.rs");
            if prelude_file.exists() {
                if !quiet {
                    println!("Executing prelude from {prelude_file:?}");
                }
                let prelude = std::fs::read_to_string(prelude_file)?;
                outputs.merge(self.execute(&prelude)?);
            }
        }
        Ok(outputs)
    }

    fn execute_command(
        &mut self,
        command: &CommandCall,
        segment: &Segment,
        state: &mut Cont