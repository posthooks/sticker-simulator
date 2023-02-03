
// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Sadly, `syn` and friends only handle valid rust -- or at least, `syn` tells
//! us that an input is invalid, but cannot determine the difference between
//! inputs that are invalid due to incompleteness and those that are completely
//! invalid as it stands. (This seems due to the fact that rust's lexical
//! primitive is not the token, but the token *tree*, which guarantees brackets
//! are properly nested.
//!
//! I looked around and short of writing a parser, nothing on crates.io helps.
//! So, this is a minimal scanner that attempts to find if input is obviously
//! unclosed. It handles:
//!
//! - Various kinds of brackets: `()`, `[]`, `{}`.
//!
//! - Strings, including raw strings with preceeding hashmarks (e.g.
//!   `r##"foo"##`)
//!
//! - Comments, including nested block comments. `/*/` is (properly) not treated
//!   as a self-closing comment, but the opening of another nesting level.
//!
//! - char/byte literals, but not very well, as they're confusing with
//!   lifetimes. It does just well enough to know that '{' doesn't open a curly
//!   brace, or to get tripped up by '\u{}'
//!
//! It doesn't handle
//!
//! - Closure arguments like `|x|`.
//!
//! - Generic paremeters like `<` (it's possible we could catch them in the
//!   turbofish case but probably not worth it).
//!
//! - Incomplete expressions/statements which aren't inside some other of a
//!   nesting, e.g. `foo +` is clearly incomplete, but we don't detect it unless
//!   it has parens around it.
//!
//! In general the goal here was to parse enough not to get confused by cases
//! that would lead us to think complete input was incomplete. This requires
//! handling strings, comments, etc, as they are allowed to have a "{" in it
//! which we'd otherwise think keeps the whole line open.
//!
//! Note that from here, it should be possible to use syn to actually parse
//! things, but that's left alone for now.

use std::iter::Peekable;
use std::str::CharIndices;
use unicode_xid::UnicodeXID;

/// Return type for `validate_source_fragment`
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FragmentValidity {
    /// Note that despite it's name, this really just means "not obviously
    /// invalid". There are many ways the source might still be invalid or
    /// incomplete that we fail to detect, but that's a limitation of the fact
    /// that we don't actually understand the source beyond purely lexical
    /// information.
    Valid,
    /// This generally means that we see a problem, and believe that, as it
    /// currently stands, additional input is not going to fix the problem. For
    /// example, mismatched braces and the like.
    ///
    /// At the moment we just send your input to rustc right away if we see
    /// this, but the UX is a bit awkward here, as it can mean we send the input
    /// off before you expect, but this seems likely to require changes to
    /// rustyline.
    Invalid,
    /// The input seems good enough, but incomplete. There's some sort of
    /// obvious indication the source is incomplete: an unclosed string quote,
    /// some sort of bracket, etc. It's pretty important that we avoid saying
    /// the source is incomplete when it's actually complete (as this would
    /// prevent the user from submitting.
    Incomplete,
}

/// Determine if a piece of source is valid, invalid, or merely incomplete. This
/// is approximate, see the module comment for details. The intent is for
/// - Incomplete to be used to mean "keep a multiline block going"
/// - Valid to mean "finishing a multiline block is allowed"
/// - and Invalid to mean something fuzzy like "wait for the user to finish the
///   current line, then send to rustc and give an error".
///     - Ideally, we'd indicate some kinds of invalidity to the user before
///       submitting -- it can be pretty surprising to be in the middle of a
///       function, add one-too-many closing parens to a nested function call,
///       and have the whole (obviously incomplete) source get sent off to the
///       compiler.
pub fn validate_source_fragment(source: &str) -> FragmentValidity {
    use Bracket::*;
    let mut stack: Vec<Bracket> = vec![];
    // The expected depth `stack` should have after the closing ']' of the attribute
    // is read; None if closing ']' has already been read or currently not reading
    // attribute
    let mut attr_end_stack_depth: Option<usize> = None;
    // Whether the item after an attribute is expected; is set to true after the
    // expected attr_end_stack_depth was reached
    let mut expects_attr_item = false;

    let mut input = source.char_indices().peekable();
    while let Some((i, c)) = input.next() {
        // Whether the next char is the start of an attribute target; for simplicity this
        // is initially set to true and only set below to false for chars which are not
        // an attribute target, such as comments and whitespace
        let mut is_attr_target = true;

        match c {
            // Possibly a comment.
            '/' => match input.peek() {
                Some((_, '/')) => {
                    eat_comment_line(&mut input);
                    is_attr_target = false;
                }
                Some((_, '*')) => {
                    input.next();
                    if !eat_comment_block(&mut input) {
                        return FragmentValidity::Incomplete;
                    }
                    is_attr_target = false;
                }
                _ => {}
            },
            '(' => stack.push(Round),
            '[' => stack.push(Square),
            '{' => stack.push(Curly),
            ')' | ']' | '}' => {
                match (stack.pop(), c) {
                    (Some(Round), ')') | (Some(Curly), '}') => {
                        // good.
                    }
                    (Some(Square), ']') => {
                        if let Some(end_stack_depth) = attr_end_stack_depth {
                            // Check if end of attribute has been reached
                            if stack.len() == end_stack_depth {
                                attr_end_stack_depth = None;
                                expects_attr_item = true;
                                // Prevent considering ']' as attribute target, and therefore
                                // directly setting `expects_attr_item = false` again below
                                is_attr_target = false;
                            }
                        }

                        // for non-attribute there is nothing else to do
                    }
                    _ => {
                        // Either the bracket stack was empty or mismatched. In
                        // the future, we should distinguish between these, and
                        // for a bracket mismatch, highlight it in the prompt
                        // somehow. I think this will require changes to
                        // `rustyline`, though.
                        return FragmentValidity::Invalid;
                    }
                }
            }
            '\'' => {
                // A character or a lifetime.
                match eat_char(&mut input) {
                    Some(EatCharRes::SawInvalid) => {
                        return FragmentValidity::Invalid;
                    }
                    Some(_) => {
                        // Saw something valid. These two cases are currently
                        // just to verify eat_char behaves as expected in tests
                    }
                    None => {
                        return FragmentValidity::Incomplete;
                    }
                }
            }
            // Start of a string.
            '\"' => {
                if let Some(sane_start) = check_raw_str(source, i) {
                    if !eat_string(&mut input, sane_start) {
                        return FragmentValidity::Incomplete;
                    }
                } else {
                    return FragmentValidity::Invalid;
                }
            }
            // Possibly an attribute.
            '#' => {
                // Only handle outer attribute (`#[...]`); for inner attribute (`#![...]`) there is
                // no need to report Incomplete because the enclosing item to which the attribute
                // applies (e.g. a function) is probably already returning Incomplete, if necessary
                if let Some((_, '[')) = input.peek() {
                    attr_end_stack_depth = Some(stack.len());
                    // Don't consume '[' here, let the general bracket handling code above do that
                }
            }
            _ => {
                // This differs from Rust grammar which only considers `Pattern_White_Space`
                // (see https://doc.rust-lang.org/reference/whitespace.html), whereas `char::is_whitespace`
                // checks for `White_Space` char property; but might not matter in most cases
                if c.is_whitespace() {
                    is_attr_target = false;
                }
            }
        }

        if is_attr_target {
            expects_attr_item = false;
        }
    }
    // Seems good to me if we get here!
    if stack.is_empty() && !expects_attr_item {
        FragmentValidity::Valid
    } else {
        FragmentValidity::Incomplete
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum StrKind {
    /// Normal string. Closed on first ", but a backslash can escape a single
    /// quote.
    Normal,
    /// Raw string. Closed after we see a " followed by the right num of
    /// hashes,
    RawStr { hashes: usize },
}

/// `quote_idx` should point at the byte index of the starting double-quote of a
/// string.
///
/// Returns the kind of string that starts at `quote_idx`, or None if we don't
/// seem to have a valid string.
fn check_raw_str(s: &str, quote_idx: usize) -> Option<StrKind> {
    use StrKind::*;
    debug_assert_eq!(s.as_bytes()[quote_idx], b'"');
    let sb = s.as_bytes();
    let index_back =
        |idx: usize| -> Option<u8> { quote_idx.checked_sub(idx).and_then(|i| sb.get(i).copied()) };
    match index_back(1) {
        // Raw string, no hashes.
        Some(b'r') => Some(RawStr { hashes: 0 }),
        Some(b'#') => {
            let mut count = 1;
            loop {
                let c = index_back(1 + count);
                match c {
                    Some(b'#') => count += 1,
                    Some(b'r') => break,
                    // Syntax error?
                    _ => return None,
                }
            }
            Some(RawStr { hashes: count })
        }
        _ => Some(Normal),
    }
}

/// Expects to be called after `iter` has consumed the starting \". Returns true
/// if the string was closed.
fn eat_string(iter: &mut Peekable<CharIndices<'_>>, kind: StrKind) -> bool {
    let (closing_hashmarks, escapes_allowed) = match kind {
        StrKind::Normal => (0, true),
        StrKind::RawStr { hashes } => (hashes, false),
    };

    while let Some((_, c)) = iter.next() {
        match c {
            '"' => {
                if closing_hashmarks == 0 {
                    return true;
                }
                let mut seen = 0;
                while let Some((_, '#')) = iter.peek() {
                    iter.next();
                    seen += 1;
                    if seen == closing_hashmarks {
                        return true;
                    }
                }
            }
            '\\' if escapes_allowed => {
                // Consume whatever is next -- but whatever it was doesn't
                // really matter to us.
                iter.next();
            }
            _ => {}
        }
    }
    false
}

/// Expects to be called after `iter` has *fully consumed* the initial `//`.
///
/// Consumes the entire comment, including the `\n`.
fn eat_comment_line<I: Iterator<Item = (usize, char)>>(iter: &mut I) {
    for (_, c) in iter {
        if c == '\n' {
            break;
        }
    }
}

/// Expects to be called after `iter` has *fully consumed* the initial `/*`
/// already. returns `true` if it scanned a fully valid nesting, and false
/// otherwise.
fn eat_comment_block(iter: &mut Peekable<CharIndices<'_>>) -> bool {
    let mut depth = 1;
    while depth != 0 {
        let Some(next) = iter.next() else { return false; };
        let c = next.1;
        match c {
            '/' => {
                if let Some((_, '*')) = iter.peek() {
                    iter.next();
                    depth += 1;
                }
            }
            '*' => {