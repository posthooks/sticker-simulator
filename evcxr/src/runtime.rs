// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::errors::bail;
use crate::errors::Error;
use once_cell::sync::OnceCell;
use regex::Regex;
use std::io;
use std::marker::PhantomData;
use std::rc::Rc;
use std::{self};

pub(crate) const EVCXR_IS_RUNTIME_VAR: &str = "EVCXR_IS_RUNTIME";
pub(crate) const EVCXR_EXECUTION_COMPLETE: &str = "EVCXR_EXECUTION_COMPLETE";

/// Binaries can call this just after staring. If w