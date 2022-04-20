// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

// This file is both a module of evcxr and is included via include_str! then
// built as a crate itself. The latter is the primary use-case. It's included as
// a submodule only so that constants can be shared.

pub const VARIABLE_CHANGED_TYPE: &str = "EVCXR_VARIABLE_CHANGED_TYPE:";
pub const USER_ERROR_OCCURRED: &str = "EVCXR_ERROR_OCCURRED";

pub struct VariableStore {
    variables: std::collections::HashMap<String, Box<dyn std::any::Any + 'static>>,
}

impl VariableStore {
    pub fn new() -> VariableStore {
        VariableStore {
            variables: std::collections::HashMap::new(),
        }
    }

    pub fn put_variable<T: 'static>(&mut self, name: &str, value: T) {
        self.variables.insert(name.to_owned(), Box::new(value));
    }

    pub fn check_variable<T: 'static>(&mut self, name: &str) -> bool {
        if let Some(v) = self.variables.get(