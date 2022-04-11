
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