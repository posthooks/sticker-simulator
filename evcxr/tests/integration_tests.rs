// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use evcxr::CommandContext;
use evcxr::Error;
use evcxr::EvalContext;
use evcxr::EvalContextOutputs;
use once_cell::sync::OnceCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io;
use std::ops::Deref;
use std::ops::DerefMut;
use std::sync::Mutex;

#[track_caller]
fn eval_and_unwrap(ctxt: &mut CommandContext, code: &str) -> HashMap<String, String> {
    match ctxt.execute(code) {
        Ok(output) => output.content_by_mime_type,
        Err(err) => {
            println!(
                "======== last src ========\n{}==========================",
                ctxt.last_source().unwrap()
            );
            match err {
                Error::CompilationErrors(errors) => {
                    for error in errors {
                        println!("{}", error.rendered());
                    }
                }
                other => println!("{}", other),
            }

            panic!("Unexpected compilation error. See above for details");
        }
    }
}

macro_rules! eval {
    ($ctxt:expr, $($t:tt)*) => {eval_and_unwrap(&mut $ctxt, stringify!($($t)*))}
}

fn new_command_context_and_outputs() -> (CommandContext, EvalContextOutputs) {
    let (eval_context, outputs) = EvalContext::new_for_testing();
    let command_context = CommandContext::with_eval_context(eval_context);
    (command_context, outputs)
}

fn send_output<T: io::Write + Send + 'static>(
    channel: crossbeam_channel::Receiver<String>,
    mut output: T,
) {
    std::thread::spawn(move || {
        while let Ok(line) = channel.recv() {
            if writeln!(output, "{}", line).is_err() {
                break;
            }
        }
    });
}
fn context_pool() -> &'static Mutex<Vec<CommandContext>> {
    static CONTEXT_POOL: OnceCell<Mutex<Vec<CommandContext>>> = OnceCell::new();
    CONTEXT_POOL.get_or_init(|| Mutex::new(vec![]))
}

struct ContextHolder {
    // Only `None` while being dropped.
    ctx: Option<CommandContext>,
}

impl Drop for ContextHolder {
    fn drop(&mut self) {
        if is_context_pool_enabled() {
            let mut pool = context_pool().lock().unwrap();
            let mut ctx = self.ctx.take().unwrap();
            ctx.reset_config();
            ctx.execute(":clear").unwrap();
            pool.push(ctx)
        }
    }
}

impl Deref for ContextHolder {
    type Target = CommandContext;

    fn deref(&self) -> &Self::Target {
        self.ctx.as_ref().unwrap()
    }
}

impl DerefMut for ContextHolder {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.ctx.as_mut().unwrap()
    }
}

fn is_context_pool_enabled() -> bool {
    std::env::var("EVCXR_DISABLE_CTX_POOL")
        .map(|var| var != "1")
        .unwrap_or(true)
}

/// Returns a ContextHolder, which will dereference to a CommandContext. When
/// the ContextHolder is dropped, the held CommandContext will be cleared then
/// returned to a global pool. This reuse speeds up running lots of tests by at
/// least 25%. This is probably mostly due to avoiding the need to reload the
/// standard library in rust-analyzer, as that is quite expensive. If you think
/// a test is causing subsequent tests to misbehave, you can disable the pool by
/// setting `EVCXR_DISABLE_CTX_POOL=1`. This can be helpful for debugging,
/// however the interference problem should be fixed as the ":clear" command,
/// combined with resetting configuration should really be sufficient to ensure
/// that subsequent tests will pass.
fn new_context() -> ContextHolder {
    let ctx = context_pool().lock().unwrap().pop().unwrap_or_else(|| {
        let (context, outputs) = new_command_context_and_outputs();
        send_output(outputs.stderr, io::stderr());
        context
    });
    ContextHolder { ctx: Some(ctx) }
}

fn defined_item_names(eval_context: &CommandContext) -> Vec<&str> {
    let mut defined_names = eval_context.defined_item_names().collect::<Vec<_>>();
    defined_names.sort();
    defined_names
}

fn variable_names_and_types(ctx: &CommandContext) -> Vec<(&str, &str)> {
    let mut var_names = ctx.variables_and_types().collect::<Vec<_>>();
    var_names.sort();
    var_names
}

fn variable_names(ctx: &CommandContext) -> Vec<&str> {
    let mut var_names = ctx
        .variables_and_types()
        .map(|(var_name, _)| var_name)
        .collect::<Vec<_>>();
    var_names.sort();
    var_names
}

#[test]
fn single_statement() {
    let mut e = new_context();
    eval!(e, assert_eq!(40i32 + 2, 42));
}

#[test]
fn save_and_restore_variables() {
    let mut e = new_context();

    eval!(e, let mut a = 34; let b = 8;);
    eval!(e, a = a + b;);
    assert_eq!(eval!(e, a), text_plain("42"));
    // Try to change a mutable variable and check that the error we get is what we expect.
    match e.execute("b = 2;") {
        Err(Error::CompilationErrors(errors)) => {
            if errors.len() != 1 {
                println!("{:#?}", errors);
            }
            assert_eq!(errors.len(), 1);
            if errors[0].code() != Some("E0594") && errors[0].code() != Some("E0384") {
                panic!("Unexpected error {:?}", errors[0].code());
     