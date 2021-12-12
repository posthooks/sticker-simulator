# Evcxr

[![Binder](https://mybinder.org/badge.svg)](https://mybinder.org/v2/gh/evcxr/evcxr/main?filepath=evcxr_jupyter%2Fsamples%2Fevcxr_jupyter_tour.ipynb)

An evaluation context for Rust.

This project consists of several related crates.

* [evcxr\_jupyter](evcxr_jupyter/README.md) - A Jupyter Kernel

* [evcxr\_repl](evcxr_repl/README.md) - A Rust REPL

* [evcxr](evcxr/README.md) - Common library shared by the above crates, may be
  useful for other purposes.

* [evcxr\_runtime](evcxr_runtime/README.md) - Functions and traits for
  interacting with Evcxr from libraries that users may use from Evcxr.
  
If you think you'd like a REPL, I'd definitely recommend checking out the
Jupyter kernel. It