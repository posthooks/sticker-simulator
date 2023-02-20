// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#[cfg(feature = "bytes")]
extern crate base64;

pub trait Display {
    /// Implementation should emit a representation of itself in one or mime
    /// types  using the functions below.
    fn evcxr_display(&self);
}

/// Represents a mime type for some content that is yet to be emitted.
pub struct ContentMimeType {
    mime_type: String,
}

/// Prepares to output some content with the specified mime type.
/// ```
/// evcxr_runtime::mime_type("text/plain").text("Hello world");
/// ```
pub fn mime_type<S: Into<String>>(mim