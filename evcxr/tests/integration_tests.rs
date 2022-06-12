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
            }
        }
        _ => unreachable!(),
    }

    // Make sure that we can correctly determine variable types when using the
    // question mark operator.
    eval_and_unwrap(
        &mut e,
        r#"
        pub mod foo {
            pub mod bar {
                pub struct Baz {}
                impl Baz {
                    pub fn r42(&self) -> i32 {42}
                }
            }
        }
        fn create_baz() -> Result<Option<foo::bar::Baz>, i32> {
            Ok(Some(foo::bar::Baz {}))
        }
    "#,
    );
    eval_and_unwrap(&mut e, "let v1 = create_baz()?;");
    eval_and_unwrap(&mut e, "let v2 = create_baz()?;");
    assert_eq!(
        eval_and_unwrap(&mut e, "v1.unwrap().r42() + v2.unwrap().r42()"),
        text_plain("84")
    );
}

#[test]
fn missing_semicolon_on_let_stmt() {
    let mut e = new_context();
    eval_and_unwrap(&mut e, "mod foo {pub mod bar { pub struct Baz {} }}");
    match e.execute("let v1 = foo::bar::Baz {}") {
        Err(Error::CompilationErrors(e)) => {
            assert!(e.first().unwrap().message().contains(';'));
        }
        x => {
            panic!("Unexpected result: {:?}", x);
        }
    }
}

#[test]
fn printing() {
    let (mut e, outputs) = new_command_context_and_outputs();

    eval!(e,
        println!("This is stdout");
        eprintln!("This is stderr");
        println!("Another stdout line");
        eprintln!("Another stderr line");
    );
    assert_eq!(outputs.stdout.recv(), Ok("This is stdout".to_owned()));
    assert_eq!(outputs.stderr.recv(), Ok("This is stderr".to_owned()));
    assert_eq!(outputs.stdout.recv(), Ok("Another stdout line".to_owned()));
    assert_eq!(outputs.stderr.recv(), Ok("Another stderr line".to_owned()));
}

#[test]
fn rc_refcell_etc() {
    let mut e = new_context();
    eval!(e,
        use std::cell::RefCell; use std::rc::Rc;
        let r: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        let r2: Rc<RefCell<String>> = Rc::clone(&r);
    );
    eval!(e,
        r.borrow_mut().push_str("f");
        let s = "oo";
    );
    eval!(e,
        r.borrow_mut().push_str(s);
        assert!(*r.borrow() == "foo");
    );
}

#[test]
fn define_then_call_function() {
    let mut e = new_context();
    eval!(
        e,
        pub fn bar() -> i32 {
            42
        }
    );
    eval!(
        e,
        pub fn foo() -> i32 {
            bar()
        }
        assert_eq!(foo(), 42);
    );
    assert_eq!(defined_item_names(&e), vec!["bar", "foo"]);
}

// This test has recently started failing on windows. It fails when deleting the .pdb file with an
// "access denied" error. No idea why. Perhaps in this scenario the file is still locked for some
// reason. This is a somewhat obscure test and Windows is a somewhat obscure platform, so I'll just
// disable this for now.
#[cfg(not(windows))]
#[test]
fn function_panics_with_variable_preserving() {
    // Don't allow stderr to be printed here. We don't really want to see the
    // panic stack trace when running tests.
    let (mut e, _) = new_command_context_and_outputs();
    eval_and_unwrap(
        &mut e,
        r#"
        :preserve_vars_on_panic 1
        let a = vec![1, 2, 3];
        let b = 42;
    "#,
    );
    eval!(e, panic!("Intentional panic {}", b););
    // The variable a isn't referenced by the code that panics, while the variable b implements
    // Copy, so neither should be lost.
    assert_eq!(
        eval!(e, format!("{:?}, {}", a, b)),
        text_plain("\"[1, 2, 3], 42\"")
    );
}

#[test]
fn function_panics_without_variable_preserving() {
    // Don't allow stderr to be printed here. We don't really want to see the
    // panic stack trace when running tests.
    let (mut e, _) = new_command_context_and_outputs();
    eval_and_unwrap(
        &mut e,
        r#"
        :preserve_vars_on_panic 0
        let a = vec![1, 2, 3];
        let b = 42;
    "#,
    );
    let result = e.execute(stringify!(panic!("Intentional panic {}", b);));
    if let Err(Error::SubprocessTerminated(message)) = result {
        assert!(message.contains("Subprocess terminated"));
    } else {
        panic!("Unexpected result: {:?}", result);
    }
    assert_eq!(variable_names_and_types(&e), vec![]);
    // Make sure that a compilation error doesn't bring the variables back from
    // the dead.
    assert!(e.execute("This will not compile").is_err());
    assert_eq!(variable_names_and_types(&e), vec![]);
}

// Also tests multiple item definitions in the one compilation unit.
#[test]
fn tls_implementing_drop() {
    let mut e = new_context();
    eval!(e,
        pub struct Foo {}
        impl Drop for Foo {
            fn drop(&mut self) {
                println!("Dropping Foo");
            }
        }
        pub fn init_foo() {
            thread_local! {
                pub static FOO: Foo = Foo {};
            }
            FOO.with(|f| ())
        }
    );
    eval!(e, init_foo(););
}

fn text_plain(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("text/plain".to_owned(), content.to_owned());
    map
}

#[test]
fn moved_value() {
    let mut e = new_context();
    eval!(e, let a = Some("foo".to_owned()););
    asser