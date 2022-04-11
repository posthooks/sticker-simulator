
// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::code_block::count_columns;
use crate::code_block::CodeBlock;
use crate::code_block::CodeKind;
use crate::code_block::CommandCall;
use crate::code_block::Segment;
use crate::code_block::UserCodeInfo;
use ariadne::Color;
use ariadne::{ColorGenerator, Label, Report, ReportKind};
use json::JsonValue;
use json::{self};
use ra_ap_ide::TextRange;
use ra_ap_ide::TextSize;
use std::fmt;
use std::fmt::Write as _;
use std::io;
use std::ops::Range;

#[derive(Debug, Clone)]
pub struct CompilationError {
    message: String,
    pub json: JsonValue,
    pub(crate) code_origins: Vec<CodeKind>,
    spanned_messages: Vec<SpannedMessage>,
    spanned_helps: Vec<SpannedMessage>,
    level: String,
}

pub enum Theme {
    Light,
    Dark,
}

fn span_to_byte_range(source: &str, span: &Span) -> Range<usize> {
    fn line_and_number_to_byte_offset(source: &str, line_number: usize, column: usize) -> usize {
        source
            .lines()
            .take(line_number - 1)
            .map(|x| x.len())
            .sum::<usize>()
            + column
            + line_number
            - 2
    }
    line_and_number_to_byte_offset(source, span.start_line, span.start_column)
        ..line_and_number_to_byte_offset(source, span.end_line, span.end_column)
}

impl CompilationError {
    pub fn build_report(
        &self,
        file_name: String,
        source: String,
        theme: Theme,
    ) -> Option<Report<(String, Range<usize>)>> {
        let error = self;
        if !source.is_ascii() {
            return None;
        }
        let mut builder =
            Report::build(ReportKind::Error, file_name.clone(), 0).with_message(error.message());
        let mut next_color = {
            let mut colors = ColorGenerator::new();
            move || {
                if let Color::Fixed(x) = colors.next() {
                    Color::Fixed(match theme {
                        Theme::Light => 255 - x,
                        Theme::Dark => x,
                    })
                } else {
                    unreachable!()
                }
            }
        };
        if let Some(code) = error.code() {
            builder = builder.with_code(code);
        }
        let mut notes = String::new();
        for spanned_message in error
            .spanned_messages()
            .iter()
            .chain(error.help_spanned().iter())
        {
            if let Some(span) = &spanned_message.span {
                if spanned_message.label.is_empty() {
                    continue;
                }
                builder = builder.with_label(
                    Label::new((file_name.clone(), span_to_byte_range(&source, span)))
                        .with_message(&spanned_message.label)
                        .with_color(next_color())
                        .with_order(10),
                );
            } else {
                notes.push_str(&spanned_message.label);
            }
        }
        if let Some(evcxr_notes) = evcxr_specific_notes(error) {
            builder.set_note(evcxr_notes);
        } else if !notes.is_empty() {
            builder.set_note(notes);
        }
        Some(builder.finish())
    }
}

fn evcxr_specific_notes(error: &CompilationError) -> Option<&'static str> {
    Some(match error.code()? {
        "E0384" | "E0596" => {
            "You can change an existing variable to mutable like: `let mut x = x;`"
        }
        _ => return None,
    })
}

fn spans_in_local_source(span: &JsonValue) -> Option<&JsonValue> {
    if let Some(file_name) = span["file_name"].as_str() {
        if file_name.ends_with("lib.rs") {
            return Some(span);
        }
    }
    let expansion = &span["expansion"];
    if expansion.is_object() {
        return spans_in_local_source(&expansion["span"]);
    }
    None
}

fn get_code_origins_for_span<'a>(
    span: &JsonValue,
    code_block: &'a CodeBlock,
) -> Vec<(&'a CodeKind, usize)> {
    let mut code_origins = Vec::new();
    if let Some(span) = spans_in_local_source(span) {
        if let (Some(line_start), Some(line_end)) =
            (span["line_start"].as_usize(), span["line_end"].as_usize())
        {
            for line in line_start..=line_end {
                code_origins.push(code_block.origin_for_line(line));
            }
        }
    }
    code_origins
}

fn get_code_origins<'a>(json: &JsonValue, code_block: &'a CodeBlock) -> Vec<&'a CodeKind> {
    let mut code_origins = Vec::new();
    if let JsonValue::Array(spans) = &json["spans"] {
        for span in spans {
            code_origins.extend(
                get_code_origins_for_span(span, code_block)
                    .iter()
                    .map(|(origin, _)| origin),
            );
        }
    }
    code_origins
}

impl CompilationError {
    pub(crate) fn opt_new(mut json: JsonValue, code_block: &CodeBlock) -> Option<CompilationError> {
        // From Cargo 1.36 onwards, errors emitted as JSON get wrapped by Cargo.
        // Retrive the inner message emitted by the compiler.
        if json["message"].is_object() {
            json = json["message"].clone();
        }
        let mut code_origins = get_code_origins(&json, code_block);
        let mut user_error_json = None;
        if let JsonValue::Array(children) = &json["children"] {
            for child in children {
                let child_origins = get_code_origins(child, code_block);
                if !code_origins.iter().any(|k| k.is_user_supplied())
                    && child_origins.iter().any(|k| k.is_user_supplied())
                {
                    // Use the child instead of the top-level error.
                    user_error_json = Some(child.clone());
                    code_origins = child_origins;
                    break;
                } else {
                    code_origins.extend(child_origins);
                }
            }
        }
        if let Some(user_error_json) = user_error_json {
            json = user_error_json;
        }

        let message = json["message"].as_str()?;
        if message.starts_with("aborting due to")
            || message.starts_with("For more information about")
            || message.starts_with("Some errors occurred")
        {
            return None;
        }
        let message = sanitize_message(message);

        Some(CompilationError {
            spanned_messages: build_spanned_messages(&json, code_block),
            spanned_helps: {
                if let JsonValue::Array(children) = &json["children"] {
                    children
                        .iter()
                        .flat_map(|x| build_spanned_messages(x, code_block))
                        .collect()
                } else {
                    vec![]
                }
            },
            message,
            level: json["level"].as_str().unwrap_or("").to_owned(),
            json,
            code_origins: code_origins.into_iter().cloned().collect(),
        })
    }

    pub(crate) fn fill_lines(&mut self, code_info: &UserCodeInfo) {
        for spanned_message in self.spanned_messages.iter_mut() {
            if let Some(span) = &spanned_message.span {
                spanned_message.lines.extend(
                    code_info.original_lines[span.start_line - 1..span.end_line]
                        .iter()
                        .map(|line| (*line).to_owned()),
                );
            }
        }
    }

    /// Returns a synthesized error that spans the specified portion of `segment`.
    pub(crate) fn from_segment_span(
        segment: &Segment,
        spanned_message: SpannedMessage,
        message: String,
    ) -> CompilationError {
        CompilationError {
            spanned_messages: vec![spanned_message],
            spanned_helps: vec![],
            message,
            json: JsonValue::Null,
            code_origins: vec![segment.kind.clone()],
            level: "error".to_owned(),
        }
    }

    /// Returns whether this error originated in code supplied by the user.
    pub fn is_from_user_code(&self) -> bool {
        self.code_origins.iter().any(CodeKind::is_user_supplied)
    }

    /// Returns whether this error originated in code that we generated.
    pub fn is_from_generated_code(&self) -> bool {
        self.code_origins.contains(&CodeKind::OtherGeneratedCode)
    }

    pub fn message(&self) -> String {
        self.message.clone()
    }

    pub fn code(&self) -> Option<&str> {
        if let JsonValue::Object(code) = &self.json["code"] {
            return code["code"].as_str();
        }
        None
    }

    pub fn explanation(&self) -> Option<&str> {
        if let JsonValue::Object(code) = &self.json["code"] {
            return code["explanation"].as_str();
        }
        None
    }

    pub fn evcxr_extra_hint(&self) -> Option<&'static str> {
        if let Some(code) = self.code() {
            Some(match code {
                "E0597" => {
                    "Values assigned to variables in Evcxr cannot contain references \
                     (unless they're static)"
                }
                _ => return None,
            })
        } else {
            None
        }
    }

    pub fn spanned_messages(&self) -> &[SpannedMessage] {
        &self.spanned_messages[..]
    }

    /// Returns the primary spanned message, or if there is no primary spanned message, perhaps
    /// because it was reported in generated code, so go filtered out, then returns the first
    /// spanned message, if any.
    pub fn primary_spanned_message(&self) -> Option<&SpannedMessage> {
        match self.spanned_messages.iter().find(|msg| msg.is_primary) {
            Some(x) => Some(x),
            None => self.spanned_messages().first(),
        }
    }

    pub fn level(&self) -> &str {
        &self.level
    }

    pub fn help_spanned(&self) -> &[SpannedMessage] {
        &self.spanned_helps
    }

    pub fn help(&self) -> Vec<String> {
        if let JsonValue::Array(children) = &self.json["children"] {
            children
                .iter()
                .filter_map(|child| {
                    if child["level"].as_str() != Some("help") {
                        return None;
                    }
                    child["message"].as_str().map(|s| {
                        let mut message = s.to_owned();
                        if let Some(replacement) =
                            child["spans"][0]["suggested_replacement"].as_str()
                        {
                            use std::fmt::Write;
                            write!(message, "\n\n{}", replacement.trim_end()).unwrap();
                        }
                        message
                    })
                })
                .collect()
        } else {
            vec![]
        }
    }

    pub fn rendered(&self) -> String {
        self.json["rendered"].as_str().unwrap_or("").to_owned()
    }
}

fn sanitize_message(message: &str) -> String {
    // Any references to `evcxr_variable_store` are beyond the end of what the
    // user typed, so we replace such references with something more meaningful.
    // This is mostly helpful with missing semicolons on let statements, which
    // produce errors such as "expected `;`, found `evcxr_variable_store`"
    message.replace("`evcxr_variable_store`", "<end of input>")
}

fn build_spanned_messages(json: &JsonValue, code_block: &CodeBlock) -> Vec<SpannedMessage> {
    let mut output_spans = Vec::new();
    let mut only_one_span = false;
    let level_label: Option<String> = (|| {
        let level = json["level"].as_str()?;
        if level != "error" {
            // We can't handle helps and notes with multiple spans currently
            only_one_span = true;
        }
        let message = json["message"].as_str()?;
        Some(format!("{level}: {message}"))