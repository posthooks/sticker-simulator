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
    /// Only present for original user code. Provides ordering 