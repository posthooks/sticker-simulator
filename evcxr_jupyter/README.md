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
already use rust-analyzer, you'll likely have this installed. To install this
using rustup, run:
```sh
rustup component add rust-src
```

## Running

To start Jupyter Notebook, run:

```sh
jupyter notebook
```

Once started, it should open a page in your web browser. Look for the "New" menu
on the right and from it, select "Rust".

## Usage information

Evcxr is both a REPL and a Jupyter kernel. See [Evcxr common
usage](https://github.com/evcxr/evcxr/blob/main/COMMON.md) for information that is common
to both.

## Custom output

The last expression in a cell gets printed. By default, we'll use the debug
formatter to emit plain text. If you'd like, you can provide a function to show
your type (or someone else's type) as HTML (or an image). To do this, the type
needs to implement a method called ```evcxr_display``` which should then print
one or more mime-typed blocks to stdout. Each block starts with a line
containing EVCXR\_BEGIN\_CONTENT followed by the mime type, then a newline, the
content then ends with a line containing EVCXR\_END\_CONTENT.

For example, the following shows how you might prov