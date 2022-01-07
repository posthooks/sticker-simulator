// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::statement_splitter;
use anyhow::anyhow;
use anyhow::Result;
use once_cell::sync::OnceCell;
use ra_ap_syntax::SyntaxNode;
use regex::Regex;
use statement_splitter::OriginalUserCode;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Segment {
    pub(crate) kind: CodeKind,
    pub(crate) code: String,
    num_lines: usize,
    /// Only present for original user code. Provides ordering and identity to the segments that
    /// came from the user.
    pub(crate) sequence: Option<usize>,
}

impl Segment {
    fn new(kind: CodeKind, mut code: String) -> Segment {
        if !code.ends_with('\n') {
            code.push('\n');
        }
        Segment {
            kind,
            num_lines: num_lines(&code),
            code,
            sequence: None,
        }
    }
}

/// Information about the code the user supplied.
pub(crate) struct UserCodeInfo<'a> {
    pub(crate) nodes: Vec<SyntaxNode>,
    pub(crate) original_lines: Vec<&'a str>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct CommandCall {
    pub(crate) command: String,
    pub(crate) args: Option<String>,
    start_byte: usize,
    pub(crate) line_number: usize,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum CodeKind {
    /// The code was supplied by the user. Errors should be reported to the user.
    OriginalUserCode(UserCodeMetadata),
    /// User code for which we don't track offsets.
    OtherUserCode,
    /// Code is packing a variable into the variable store. Failure modes include (a) incorrect type
    /// (b) variable has been moved (c) non-static lifetime.
    PackVariable {
        variable_name: String,
    },
    /// A line of code that has a fallback to be used in case the supplied line fails to compile.
    WithFallback(CodeBlock),
    /// Code that we generated, but which we don't expect errors from. If we get errors there's not
    /// much we can do besides give the user as much information as we can, apologise and ask to
    /// file a bug report.
    OtherGeneratedCode,
    /// We had trouble determining what the error applied to.
    Command(CommandCall),
    Unknown,
}

impl CodeKind {
    /// Returns whether self is a WithFallback where the replacement is equal to the supplied
    /// fallback. Us