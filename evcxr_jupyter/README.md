# Evcxr Jupyter Kernel

[![Binder](https://mybinder.org/badge.svg)](https://mybinder.org/v2/gh/evcxr/evcxr/main?filepath=evcxr_jupyter%2Fsamples%2Fevcxr_jupyter_tour.ipynb)

[![Latest Version](https://img.shields.io/crates/v/evcxr_jupyter.svg)](https://crates.io/crates/evcxr_jupyter)

A [Jupyter](https://jupyter.org/) Kernel for the Rust programming language.

## Installation

If you don't already have Rust installed, [follow these
instructions](https://www.rust-lang.org/tools/install).

You can either download a pre-built binary from the
[Releases](https://github.com/evcxr/evcxr/releases) page, extract it from the
archive and put it somewhere on your path, or build from source by running:
```sh
cargo install evcxr_jupyter
```

Whether using a prebuilt binary or one you built yourself, you'll need to run
the following command in order to register the kernel with Jupyter.

```sh
evcxr_jupyter --install
```

If your operating system is an older version, or has a different libc than what
the pre-built binaries were compiled with, then you'll need to build from source
using the command above.

To actually use evcxr_jupyter, you'll need Jupyter notbook to be installed.
* Debian or Ubuntu Linux: `sudo apt install jupyter-notebook`
* Mac: You might be able to `brew install jupyter`
* Windows, or if the above options don't work for you, see
  https://jupyter.org/install

You'll also need the source for the Rust standard library installed. If you
already use rust-analyzer, you'll likel