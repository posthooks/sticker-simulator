
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