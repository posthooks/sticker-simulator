// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::process;

// Checks that our binary can be executed. This used to be an important thing to
// check due to https://github.com/rust-lang/rust/issues/45601 whic