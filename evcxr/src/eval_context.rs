
// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::child_process::ChildProcess;
use crate::code_block::CodeBlock;
use crate::code_block::CodeKind;
use crate::code_block::Segment;
use crate::code_block::UserCodeInfo;
use crate::crate_config::ExternalCrate;
use crate::errors::bail;
use crate::errors::CompilationError;
use crate::errors::Error;
use crate::errors::Span;
use crate::errors::SpannedMessage;
use crate::evcxr_internal_runtime;
use crate::item;
use crate::module::Module;
use crate::module::SoFile;
use crate::runtime;
use crate::rust_analyzer::Completions;
use crate::rust_analyzer::RustAnalyzer;
use crate::rust_analyzer::TypeName;
use crate::rust_analyzer::VariableInfo;
use crate::use_trees::Import;
use anyhow::Result;
use once_cell::sync::OnceCell;
use ra_ap_ide::TextRange;
use ra_ap_syntax::ast;
use ra_ap_syntax::AstNode;
use ra_ap_syntax::SyntaxKind;
use ra_ap_syntax::SyntaxNode;
use regex::Regex;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

pub struct EvalContext {
    // Order is important here. We need to drop child_process before _tmpdir,
    // since if the subprocess hasn't terminted before we clean up the temporary
    // directory, then on some platforms (e.g. Windows), files in the temporary
    // directory will still be locked, so won't be deleted.
    child_process: ChildProcess,
    // Our tmpdir if EVCXR_TMPDIR wasn't set - Drop causes tmpdir to be cleaned up.
    _tmpdir: Option<tempfile::TempDir>,
    module: Module,
    committed_state: ContextState,
    stdout_sender: crossbeam_channel::Sender<String>,
    analyzer: RustAnalyzer,
    initial_config: Config,
}

#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub(crate) crate_dir: PathBuf,
    pub(crate) debug_mode: bool,
    // Whether we should preserve variables that are Copy when a panic occurs.
    // Sounds good, but unfortunately doing so currently requires an extra build
    // attempt to determine if the type of the variable is copy.
    preserve_vars_on_panic: bool,
    output_format: String,
    display_types: bool,
    /// Whether to try to display the final expression. Currently this needs to
    /// be turned off when doing tab completion or cargo check, but otherwise it
    /// should always be on.
    display_final_expression: bool,
    /// Whether to expand and deduplicate use statements. We need to be able to
    /// turn this off in order for tab-completion of use statements to work, but
    /// otherwise this should always be on.
    expand_use_statements: bool,
    opt_level: String,
    error_fmt: &'static ErrorFormat,
    /// Whether to pass -Ztime-passes to the compiler and print the result.
    /// Causes the nightly compiler, which must be installed to be selected.
    pub(crate) time_passes: bool,
    pub(crate) linker: String,
    pub(crate) sccache: Option<PathBuf>,
    /// Whether to attempt to avoid network access.
    pub(crate) offline_mode: bool,
    pub(crate) toolchain: String,
    cargo_path: String,
    pub(crate) rustc_path: String,
}

fn create_initial_config(crate_dir: PathBuf) -> Config {
    let mut config = Config::new(crate_dir);
    // default the linker to mold, then lld, first checking if either are installed
    // neither linkers support macos, so fallback to system (aka default)
    // https://github.com/rui314/mold/issues/132
    if !cfg!(target_os = "macos") && which::which("mold").is_ok() {
        config.linker = "mold".to_owned();
    } else if !cfg!(target_os = "macos") && which::which("lld").is_ok() {
        config.linker = "lld".to_owned();
    }
    config
}

impl Config {
    pub fn new(crate_dir: PathBuf) -> Self {
        Config {
            crate_dir,
            debug_mode: false,
            preserve_vars_on_panic: true,
            output_format: "{:?}".to_owned(),
            display_types: false,
            display_final_expression: true,
            expand_use_statements: true,
            opt_level: "2".to_owned(),
            error_fmt: &ERROR_FORMATS[0],
            time_passes: false,
            linker: "system".to_owned(),
            sccache: None,
            offline_mode: false,
            toolchain: String::new(),
            cargo_path: default_cargo_path(),
            rustc_path: default_rustc_path(),
        }
    }

    pub fn set_sccache(&mut self, enabled: bool) -> Result<(), Error> {
        if enabled {
            if let Ok(path) = which::which("sccache") {
                self.sccache = Some(path);
            } else {
                bail!("Couldn't find sccache. Try running `cargo install sccache`.");
            }
        } else {
            self.sccache = None;
        }
        Ok(())
    }

    pub fn sccache(&self) -> bool {
        self.sccache.is_some()
    }

    pub(crate) fn cargo_command(&self, command_name: &str) -> Command {
        let mut command = if self.linker == "mold" {
            Command::new("mold")
        } else {
            Command::new(&self.cargo_path)
        };
        if self.linker == "mold" {
            command.arg("-run").arg(&self.cargo_path);
        }
        if !self.toolchain.is_empty() {
            command.arg(format!("+{}", self.toolchain));
        }
        if self.offline_mode {
            command.arg("--offline");
        }
        command.arg(command_name);
        command.current_dir(&self.crate_dir);
        command
    }
}

#[derive(Debug)]
struct ErrorFormat {
    format_str: &'static str,
    format_trait: &'static str,
}

static ERROR_FORMATS: &[ErrorFormat] = &[
    ErrorFormat {
        format_str: "{}",
        format_trait: "std::fmt::Display",
    },
    ErrorFormat {
        format_str: "{:?}",
        format_trait: "std::fmt::Debug",
    },
    ErrorFormat {
        format_str: "{:#?}",
        format_trait: "std::fmt::Debug",
    },
];

const SEND_TEXT_PLAIN_DEF: &str = stringify!(
    fn evcxr_send_text_plain(text: &str) {
        use std::io::Write;
        use std::io::{self};
        fn try_send_text(text: &str) -> io::Result<()> {
            let stdout = io::stdout();
            let mut output = stdout.lock();
            output.write_all(b"EVCXR_BEGIN_CONTENT text/plain\n")?;
            output.write_all(text.as_bytes())?;
            output.write_all(b"\nEVCXR_END_CONTENT\n")?;
            Ok(())
        }
        if let Err(error) = try_send_text(text) {
            eprintln!("Failed to send content to parent: {:?}", error);
            std::process::exit(1);
        }
    }
);

const GET_TYPE_NAME_DEF: &str = stringify!(
    /// Shorten a type name. Convert "core::option::Option<alloc::string::String>" into "Option<String>".
    pub fn evcxr_shorten_type(t: &str) -> String {
        // This could have been done easily with regex, but we must only depend on stdlib.
        // We go over the string backwards, and remove all alphanumeric and ':' chars following a ':'.
        let mut r = String::with_capacity(t.len());
        let mut is_skipping = false;
        for c in t.chars().rev() {
            if !is_skipping {
                if c == ':' {
                    is_skipping = true;
                } else {
                    r.push(c);
                }
            } else {
                if !c.is_alphanumeric() && c != '_' && c != ':' {
                    is_skipping = false;
                    r.push(c);
                }
            }
        }
        r.chars().rev().collect()
    }

    fn evcxr_get_type_name<T>(_: &T) -> String {
        evcxr_shorten_type(std::any::type_name::<T>())
    }
);

const PANIC_NOTIFICATION: &str = "EVCXR_PANIC_NOTIFICATION";

// Outputs from an EvalContext. This is a separate struct since users may want
// destructure this and pass its components to separate threads.
pub struct EvalContextOutputs {
    pub stdout: crossbeam_channel::Receiver<String>,
    pub stderr: crossbeam_channel::Receiver<String>,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct InputRequest {
    pub prompt: String,
    pub is_password: bool,
}

pub struct EvalCallbacks<'a> {
    pub input_reader: &'a dyn Fn(InputRequest) -> String,
}

fn default_input_reader(_: InputRequest) -> String {
    String::new()
}

impl<'a> Default for EvalCallbacks<'a> {
    fn default() -> Self {
        EvalCallbacks {
            input_reader: &default_input_reader,
        }
    }
}

impl EvalContext {
    pub fn new() -> Result<(EvalContext, EvalContextOutputs), Error> {
        fix_path();

        let current_exe = std::env::current_exe()?;
        Self::with_subprocess_command(std::process::Command::new(current_exe))
    }

    #[cfg(windows)]
    fn apply_platform_specific_vars(module: &Module, command: &mut std::process::Command) {
        // Windows doesn't support rpath, so we need to set PATH so that it
        // knows where to find dlls.
        use std::ffi::OsString;
        let mut path_var_value = OsString::new();
        path_var_value.push(&module.deps_dir());
        path_var_value.push(";");

        let mut sysroot_command = std::process::Command::new("rustc");
        sysroot_command.arg("--print").arg("sysroot");
        path_var_value.push(format!(
            "{}\\bin;",
            String::from_utf8_lossy(&sysroot_command.output().unwrap().stdout).trim()
        ));
        path_var_value.push(std::env::var("PATH").unwrap_or_default());

        command.env("PATH", path_var_value);
    }

    #[cfg(not(windows))]
    fn apply_platform_specific_vars(_module: &Module, _command: &mut std::process::Command) {}

    #[doc(hidden)]
    pub fn new_for_testing() -> (EvalContext, EvalContextOutputs) {
        let testing_runtime_path = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("testing_runtime");
        let (mut context, outputs) =
            EvalContext::with_subprocess_command(std::process::Command::new(testing_runtime_path))
                .unwrap();
        let mut state = context.state();
        state.set_offline_mode(true);
        context.commit_state(state);
        (context, outputs)
    }

    pub fn with_subprocess_command(
        mut subprocess_command: std::process::Command,
    ) -> Result<(EvalContext, EvalContextOutputs), Error> {
        let mut opt_tmpdir = None;
        let tmpdir_path;
        if let Ok(from_env) = std::env::var("EVCXR_TMPDIR") {
            tmpdir_path = PathBuf::from(from_env);
        } else {
            let tmpdir = tempfile::tempdir()?;
            tmpdir_path = PathBuf::from(tmpdir.path());
            opt_tmpdir = Some(tmpdir);
        }

        let analyzer = RustAnalyzer::new(&tmpdir_path)?;
        let module = Module::new(tmpdir_path)?;

        Self::apply_platform_specific_vars(&module, &mut subprocess_command);

        let (stdout_sender, stdout_receiver) = crossbeam_channel::unbounded();
        let (stderr_sender, stderr_receiver) = crossbeam_channel::unbounded();
        let child_process = ChildProcess::new(subprocess_command, stderr_sender)?;
        let initial_config = create_initial_config(module.crate_dir().to_owned());
        let initial_state = ContextState::new(initial_config.clone());
        let mut context = EvalContext {
            _tmpdir: opt_tmpdir,
            committed_state: initial_state,
            module,
            child_process,
            stdout_sender,
            analyzer,
            initial_config,
        };
        let outputs = EvalContextOutputs {
            stdout: stdout_receiver,
            stderr: stderr_receiver,
        };
        if context.committed_state.linker() == "lld" && context.eval("42").is_err() {
            context.committed_state.set_linker("system".to_owned());
        } else {
            // We need to eval something anyway, otherwise rust-analyzer crashes when trying to get
            // completions. Not 100% sure. Just writing Cargo.toml isn't sufficient.
            if let Err(error) = context.eval("42") {
                drop(context);
                let mut stderr = String::new();
                while let Ok(line) = outputs.stderr.recv() {
                    stderr.push_str(&line);
                    stderr.push('\n');
                }
                return Err(format!("{stderr}{error}").into());
            }
        }
        context.initial_config = context.committed_state.config.clone();
        Ok((context, outputs))
    }

    /// Returns a new context state, suitable for passing to `eval` after
    /// optionally calling things like `add_dep`.
    pub fn state(&self) -> ContextState {
        self.committed_state.clone()
    }

    /// Evaluates the supplied Rust code.
    pub fn eval(&mut self, code: &str) -> Result<EvalOutputs, Error> {
        self.eval_with_state(code, self.state())
    }

    pub fn eval_with_state(
        &mut self,
        code: &str,
        state: ContextState,
    ) -> Result<EvalOutputs, Error> {
        let (user_code, code_info) = CodeBlock::from_original_user_code(code);
        self.eval_with_callbacks(user_code, state, &code_info, &mut EvalCallbacks::default())
    }

    pub(crate) fn check(
        &mut self,
        user_code: CodeBlock,
        mut state: ContextState,
        code_info: &UserCodeInfo,
    ) -> Result<Vec<CompilationError>, Error> {
        state.config.display_final_expression = false;
        state.config.expand_use_statements = false;
        let user_code = state.apply(user_code, &code_info.nodes)?;
        let code = state.analysis_code(user_code.clone());
        let errors = self.module.check(&code, &state.config)?;
        Ok(state.apply_custom_errors(errors, &user_code, code_info))
    }

    /// Evaluates the supplied Rust code.
    pub(crate) fn eval_with_callbacks(
        &mut self,
        user_code: CodeBlock,
        mut state: ContextState,
        code_info: &UserCodeInfo,
        callbacks: &mut EvalCallbacks,
    ) -> Result<EvalOutputs, Error> {
        if user_code.is_empty()
            && !self
                .committed_state
                .state_change_can_fail_compilation(&state)
        {
            self.commit_state(state);
            return Ok(EvalOutputs::default());
        }
        let mut phases = PhaseDetailsBuilder::new();
        let code_out = state.apply(user_code.clone(), &code_info.nodes)?;

        let mut outputs = match self.run_statements(code_out, &mut state, &mut phases, callbacks) {
            error @ Err(Error::SubprocessTerminated(_)) => {
                self.restart_child_process()?;
                return error;
            }
            Err(Error::CompilationErrors(errors)) => {
                let mut errors = state.apply_custom_errors(errors, &user_code, code_info);
                // If we have any errors in user code then remove all errors that aren't from user
                // code.
                if errors.iter().any(|error| error.is_from_user_code()) {
                    errors.retain(|error| error.is_from_user_code())
                }
                return Err(Error::CompilationErrors(errors));
            }
            error @ Err(_) => return error,
            Ok(x) => x,
        };

        // Once, we reach here, our code has successfully executed, so we
        // conclude that variable changes are now applied.
        self.commit_state(state);

        phases.phase_complete("Execution");
        outputs.phases = phases.phases;

        Ok(outputs)
    }

    pub(crate) fn completions(
        &mut self,
        user_code: CodeBlock,
        mut state: ContextState,
        nodes: &[SyntaxNode],
        offset: usize,
    ) -> Result<Completions> {
        // Wrapping the final expression in order to display it might interfere
        // with completions on that final expression.
        state.config.display_final_expression = false;
        // Expanding use statements would prevent us from tab-completing those
        // use statements, since we lose information about where each bit came
        // from when we expand. This could be fixed with some work, but there's
        // not really any downside to turn it off here. It'll produce errors,
        // but those errors don't effect the analysis needed for completions.
        state.config.expand_use_statements = false;
        let user_code = state.apply(user_code, nodes)?;
        let code = state.analysis_code(user_code);
        let wrapped_offset = code.user_offset_to_output_offset(offset)?;

        if state.config.debug_mode {
            let mut s = code.code_string();
            s.insert_str(wrapped_offset, "<|>");
            println!("=========\n{s}\n==========");
        }

        self.analyzer.set_source(code.code_string())?;
        let mut completions = self.analyzer.completions(wrapped_offset)?;
        completions.start_offset = code.output_offset_to_user_offset(completions.start_offset)?;
        completions.end_offset = code.output_offset_to_user_offset(completions.end_offset)?;
        // Filter internal identifiers.
        completions.completions.retain(|c| {
            c.code != "evcxr_variable_store"
                && c.code != "evcxr_internal_runtime"
                && c.code != "evcxr_analysis_wrapper"
        });
        Ok(completions)
    }

    pub fn last_source(&self) -> Result<String, std::io::Error> {
        self.module.last_source()
    }

    pub fn set_opt_level(&mut self, level: &str) -> Result<(), Error> {
        self.committed_state.set_opt_level(level)
    }

    pub fn set_time_passes(&mut self, value: bool) {
        self.committed_state.set_time_passes(value);
    }

    pub fn set_preserve_vars_on_panic(&mut self, value: bool) {
        self.committed_state.set_preserve_vars_on_panic(value);
    }

    pub fn set_error_format(&mut self, value: &str) -> Result<(), Error> {
        self.committed_state.set_error_format(value)
    }

    pub fn variables_and_types(&self) -> impl Iterator<Item = (&str, &str)> {
        self.committed_state
            .variable_states
            .iter()
            .map(|(v, t)| (v.as_str(), t.type_name.as_str()))
    }

    pub fn defined_item_names(&self) -> impl Iterator<Item = &str> {
        self.committed_state
            .items_by_name
            .keys()
            .map(String::as_str)
    }

    // Clears all state, while keeping tmpdir. This allows us to effectively
    // restart, but without having to recompile any external crates we'd already
    // compiled. Config is preserved.
    pub fn clear(&mut self) -> Result<(), Error> {
        self.committed_state = self.cleared_state();
        self.restart_child_process()
    }

    /// Returns the state that would result from clearing. Config is preserved. Nothing is done to
    /// the subprocess.
    pub(crate) fn cleared_state(&self) -> ContextState {
        ContextState::new(self.committed_state.config.clone())
    }

    pub fn reset_config(&mut self) {
        self.committed_state.config = self.initial_config.clone();
    }

    pub fn process_handle(&self) -> Arc<Mutex<std::process::Child>> {
        self.child_process.process_handle()
    }

    fn restart_child_process(&mut self) -> Result<(), Error> {
        self.committed_state.variable_states.clear();
        self.committed_state.stored_variable_states.clear();
        self.child_process = self.child_process.restart()?;
        Ok(())
    }

    pub(crate) fn last_compile_dir(&self) -> &Path {
        self.module.crate_dir()
    }

    fn commit_state(&mut self, mut state: ContextState) {
        for variable_state in state.variable_states.values_mut() {
            // This span only makes sense when the variable is first defined.
            variable_state.definition_span = None;
        }
        state.stored_variable_states = state.variable_states.clone();
        state.commit_old_user_code();
        self.committed_state = state;
    }

    fn run_statements(
        &mut self,
        mut user_code: CodeBlock,
        state: &mut ContextState,
        phases: &mut PhaseDetailsBuilder,
        callbacks: &mut EvalCallbacks,
    ) -> Result<EvalOutputs, Error> {
        self.write_cargo_toml(state)?;
        self.fix_variable_types(state, state.analysis_code(user_code.clone()))?;
        // In some circumstances we may need a few tries before we get the code right. Note that
        // we'll generally give up sooner than this if there's nothing left that we think we can
        // fix. The limit is really to prevent retrying indefinitely in case our "fixing" of things
        // somehow ends up flip-flopping back and forth. Not sure how that could happen, but best to
        // avoid any infinite loops.
        let mut remaining_retries = 5;
        // TODO: Now that we have rust analyzer, we can probably with a bit of work obtain all the
        // information we need without relying on compilation errors. See if we can get rid of this.
        loop {
            // Try to compile and run the code.
            let result = self.try_run_statements(
                user_code.clone(),
                state,
                state.compilation_mode(),
                phases,
                callbacks,
            );
            match result {
                Ok(execution_artifacts) => {
                    return Ok(execution_artifacts.output);
                }

                Err(Error::CompilationErrors(errors)) => {
                    // If we failed to compile, attempt to deal with the first
                    // round of compilation errors by adjusting variable types,
                    // whether they've been moved into the catch_unwind block
                    // etc.
                    if remaining_retries > 0 {
                        let mut fixed = HashSet::new();
                        for error in &errors {
                            self.attempt_to_fix_error(error, &mut user_code, state, &mut fixed)?;
                        }
                        if !fixed.is_empty() {
                            remaining_retries -= 1;
                            let fixed_sorted: Vec<_> = fixed.into_iter().collect();
                            phases.phase_complete(&fixed_sorted.join("|"));
                            continue;
                        }
                    }
                    if !user_code.is_empty() {
                        // We have user code and it appears to have an error, recompile without
                        // catch_unwind to try and get a better error message. e.g. we don't want the
                        // user to see messages like "cannot borrow immutable captured outer variable in
                        // an `FnOnce` closure `a` as mutable".
                        self.try_run_statements(
                            user_code,
                            state,
                            CompilationMode::NoCatchExpectError,
                            phases,
                            callbacks,
                        )?;
                    }
                    return Err(Error::CompilationErrors(errors));
                }

                Err(Error::TypeRedefinedVariablesLost(variables)) => {
                    for variable in &variables {
                        state.variable_states.remove(variable);
                        state.stored_variable_states.remove(variable);
                        self.committed_state.variable_states.remove(variable);
                        self.committed_state.stored_variable_states.remove(variable);
                    }
                    remaining_retries -= 1;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn try_run_statements(
        &mut self,
        user_code: CodeBlock,
        state: &mut ContextState,
        compilation_mode: CompilationMode,
        phases: &mut PhaseDetailsBuilder,
        callbacks: &mut EvalCallbacks,
    ) -> Result<ExecutionArtifacts, Error> {
        let code = state.code_to_compile(user_code, compilation_mode);
        let so_file = self.module.compile(&code, &state.config)?;

        if compilation_mode == CompilationMode::NoCatchExpectError {
            // Uh-oh, caller was expecting an error, return OK and the caller can return the
            // original error.
            return Ok(ExecutionArtifacts {
                output: EvalOutputs::new(),
            });
        }
        phases.phase_complete("Final compile");

        let output = self.run_and_capture_output(state, &so_file, callbacks)?;
        Ok(ExecutionArtifacts { output })
    }

    pub(crate) fn write_cargo_toml(&self, state: &ContextState) -> Result<()> {
        self.module.write_cargo_toml(state)?;
        self.module.write_config_toml(state)?;
        Ok(())
    }

    fn fix_variable_types(
        &mut self,
        state: &mut ContextState,
        code: CodeBlock,
    ) -> Result<(), Error> {
        self.analyzer.set_source(code.code_string())?;
        for (
            variable_name,
            VariableInfo {
                type_name,
                is_mutable,
            },
        ) in self.analyzer.top_level_variables("evcxr_analysis_wrapper")
        {
            // We don't want to try to store record evcxr_variable_store into itself, so we ignore
            // it.
            if variable_name == "evcxr_variable_store" {
                continue;
            }
            let type_name = match type_name {
                TypeName::Named(x) => x,
                TypeName::Closure => bail!(
                    "The variable `{}` is a closure, which cannot be persisted.\n\
                     You can however persist closures if you box them. e.g.:\n\
                     let f: Box<dyn Fn()> = Box::new(|| {{println!(\"foo\")}});\n\
                     Alternatively, you can prevent evcxr from attempting to persist\n\
                     the variable by wrapping your code in braces.",
                    variable_name
                ),
                TypeName::Unknown => bail!(
                    "Couldn't automatically determine type of variable `{}`.\n\
                     Please give it an explicit type.",
                    variable_name
                ),
            };
            // For now, we need to look for and escape any reserved words. This should probably in
            // theory be done in rust analyzer in a less hacky way.
            let type_name = replace_reserved_words_in_type(&type_name);
            state
                .variable_states
                .entry(variable_name)
                .or_insert_with(|| VariableState {
                    type_name: String::new(),
                    is_mut: is_mutable,
                    move_state: VariableMoveState::New,
                    definition_span: None,
                })
                .type_name = type_name;
        }
        Ok(())
    }

    fn run_and_capture_output(
        &mut self,
        state: &mut ContextState,
        so_file: &SoFile,
        callbacks: &mut EvalCallbacks,
    ) -> Result<EvalOutputs, Error> {
        let mut output = EvalOutputs::new();
        // TODO: We should probably send an OsString not a String. Otherwise
        // things won't work if the path isn't UTF-8 - apparently that's a thing
        // on some platforms.
        let fn_name = state.current_user_fn_name();
        self.child_process.send(&format!(
            "LOAD_AND_RUN {} {}",
            so_file.path.to_string_lossy(),
            fn_name,
        ))?;

        state.build_num += 1;

        let mut got_panic = false;
        let mut lost_variables = Vec::new();
        static MIME_OUTPUT: OnceCell<Regex> = OnceCell::new();
        let mime_output =
            MIME_OUTPUT.get_or_init(|| Regex::new("EVCXR_BEGIN_CONTENT ([^ ]+)").unwrap());
        loop {
            let line = self.child_process.recv_line()?;
            if line == runtime::EVCXR_EXECUTION_COMPLETE {
                break;
            }
            if line == PANIC_NOTIFICATION {
                got_panic = true;
            } else if line.starts_with(evcxr_input::GET_CMD) {
                let is_password = line.starts_with(evcxr_input::GET_CMD_PASSWORD);
                let prompt = line.split(':').nth(1).unwrap_or_default().to_owned();
                self.child_process
                    .send(&(callbacks.input_reader)(InputRequest {
                        prompt,
                        is_password,
                    }))?;
            } else if line == evcxr_internal_runtime::USER_ERROR_OCCURRED {
                // A question mark operator in user code triggered an early
                // return. Any newly defined variables won't have been stored.
                state
                    .variable_states
                    .retain(|_variable_name, variable_state| {
                        variable_state.move_state != VariableMoveState::New
                    });
            } else if let Some(variable_name) =
                line.strip_prefix(evcxr_internal_runtime::VARIABLE_CHANGED_TYPE)
            {
                lost_variables.push(variable_name.to_owned());
            } else if let Some(captures) = mime_output.captures(&line) {
                let mime_type = captures[1].to_owned();
                let mut content = String::new();
                loop {
                    let line = self.child_process.recv_line()?;
                    if line == "EVCXR_END_CONTENT" {
                        break;
                    }
                    if line == PANIC_NOTIFICATION {
                        got_panic = true;
                        break;
                    }
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str(&line);
                }
                output.content_by_mime_type.insert(mime_type, content);
            } else {
                // Note, errors sending are ignored, since it just means the
                // user of the library has dropped the Receiver.
                let _ = self.stdout_sender.send(line);
            }
        }
        if got_panic {
            state
                .variable_states
                .retain(|_variable_name, variable_state| {
                    variable_state.move_state != VariableMoveState::New
                });
        } else if !lost_variables.is_empty() {
            return Err(Error::TypeRedefinedVariablesLost(lost_variables));
        }
        Ok(output)
    }

    fn attempt_to_fix_error(
        &mut self,
        error: &CompilationError,
        user_code: &mut CodeBlock,
        state: &mut ContextState,
        fixed_errors: &mut HashSet<&'static str>,
    ) -> Result<(), Error> {
        for code_origin in &error.code_origins {
            match code_origin {
                CodeKind::PackVariable { variable_name } => {
                    if error.code() == Some("E0382") {
                        // Use of moved value.
                        state.variable_states.remove(variable_name);
                        fixed_errors.insert("Captured value");
                    } else if error.code() == Some("E0425") {
                        // cannot find value in scope.
                        state.variable_states.remove(variable_name);
                        fixed_errors.insert("Variable moved");
                    } else if error.code() == Some("E0603") {
                        if let Some(variable_state) = state.variable_states.remove(variable_name) {
                            bail!(
                                "Failed to determine type of variable `{}`. rustc suggested type \
                             {}, but that's private. Sometimes adding an extern crate will help \
                             rustc suggest the correct public type name, or you can give an \
                             explicit type.",
                                variable_name,
                                variable_state.type_name
                            );