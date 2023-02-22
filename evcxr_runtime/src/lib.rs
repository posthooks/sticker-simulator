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
pub fn mime_type<S: Into<String>>(mime_type: S) -> ContentMimeType {
    ContentMimeType {
        mime_type: mime_type.into(),
    }
}

impl ContentMimeType {
    /// Emits the supplied content, which should be of the mime type already
    /// specified. If the type is a binary format (e.g. image/png), the content
    /// should have already been base64 encoded.
    /// ```
    /// evcxr_runtime::mime_type("text/html")
    ///     .text("<span style=\"color: red\">>Hello world</span>");
    /// ```
    pub fn text<S: AsRef<str>>(self, text: S) {
        println!(
            "EVCXR_BEGIN_CONTENT {}\n{}\nEVCXR_END_CONTENT",
            self.mime_type,
            text.as_ref()
        );
    }

    /// Emits the supplied content, which should be