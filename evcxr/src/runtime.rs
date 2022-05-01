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

/// Binaries can call this just after staring. If we detect that we're actually
/// running as a subprocess, control will not return.
pub fn runtime_hook() {
    if std::env::var(EVCXR_IS_RUNTIME_VAR).is_ok() {
        Runtime::new().run_loop();
    }
}

struct Runtime {
    shared_objects: Vec<libloading::Library>,
    variable_store_ptr: *mut std::os::raw::c_void,
    // Our variable store is permitted to contain non-Send types (e.g. Rc), therefore we need to be
    // non-Send as well.
    _phantom_rc: PhantomData<Rc<()>>,
}

impl Runtime {
    fn new() -> Runtime {
        Runtime {
            shared_objects: Vec::new(),
            variable_store_ptr: std::ptr::null_mut(),
            _phantom_rc: PhantomData,
        }
    }

    fn run_loop(&mut self) -> ! {
        use std::io::BufRead;

        self.install_crash_handlers();

        let stdin = std::io::stdin();
        #[allow(unknown_lints, clippy::significant_drop_in_scrutinee)]
        for line in stdin.lock().lines() {
            if let Err(error) = self.handle_line(&line) {
                eprintln!("While processing instruction `{line:?}`, got error: {error:?}",);
                std::process::exit(99);
            }
        }
        std::process::exit(0);
    }

    fn handle_line(&mut self, line: &io::Result<String>) -> Result<(), Error> {
        let line = line.as_ref()?;
        static LOAD_AND_RUN: OnceCell<Regex> = OnceCell::new();
        let load_and_run =
            LOAD_AND_RUN.get_or_init(|| Regex::new("LOAD_AND_RUN ([^ ]+) ([^ ]+)").unwrap());
        if let Some(captures) = load_and_run.captures(line) {
            self.load_and_run(&captures[1], &captures[2])
        } else {
            bail!("Unrecognised line: {}", line);
        }
    }

    fn load_and_run(&mut self, so_path: &str, fn_name: &str) -> Result<(), Error> {
        use std::os::raw::c_void;
        let shared_object = unsafe { libloading::Library::new(so_path) }?;
        unsafe {
            let user_fn = shared_object
                .get::<extern "C" fn(*mut c_void) -> *mut c_void>(fn_name.as_bytes())?;
            self.variable_store_ptr = user_fn(self.variable_store_ptr);
        }
        println!("{EVCXR_EXECUTION_COMPLETE}");
        self.shared_objects.push(shared_object);
        Ok(())
    }

    #[cfg(all(unix, not(target_os = "freebsd")))]
    pub fn install_crash_handlers(&self) {
        use backtrace::Backtrace;
        use sig::ffi::Sig;
        extern "C" fn segfault_handler(signal: i32) {
            eprintln!(
                "{}",
                match signal {
                    Sig::SEGV => "Segmentation fault.",
                    Sig::ILL => "Illegal instruction.",
                    Sig::BUS => "Bus error.",
           