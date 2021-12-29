# Evcxr library

[![Latest Version](https://img.shields.io/crates/v/evcxr.svg)](https://crates.io/crates/evcxr)

An implementation of eval() for Rust.

The main struct in this crate is ```EvalContext```. You create one, then ask it
to eval bits of code. Any defined functions, variables etc are local to that
context.

To see an example usage, see
[examples/example_eval.rs](examples/example_eval.rs).

I'll not go into too much detail here, since the purpose of this library is
really to provide functionality to evcxr\_jupyter and evcxr\_repl. If you'd like
to try using this 