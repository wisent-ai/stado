//! The lexical steps the Tuist manifest reader needs, over characters rather
//! than bytes so a non-ASCII letter inside a string literal cannot split a
//! step.
//!
//! Brackets are matched with a stack, string literals and comments are
//! stepped over, and anything that does not balance is an error: a shorter
//! surface reads to the rule as a removed capability, so a manifest this
//! reader cannot follow must fail rather than yield fewer names.

pub(super) mod literal;

use regex::Regex;

use crate::primitives::tally::Tally;

/// Every failure is a sentence naming the file and what could not be read.
pub(crate) type Read<T> = Result<T, String>;

const QUOTE: char = '"';
const ESCAPE: char = '\\';
const MULTILINE_QUOTE: &str = "\"\"\"";
const LINE_COMMENT: &str = "//";
const BLOCK_OPEN: &str = "/*";
const BLOCK_CLOSE: &str = "*/";

pub(crate) fn chars(text: &str) -> Vec<char> {
    text.chars().collect()
}

pub(crate) fn string(text: &[char]) -> String {
    text.iter().collect()
}

/// `text` without surrounding whitespace.
pub(crate) fn trimmed(text: &[char]) -> &[char] {
    let start = text
        .iter()
        .take_while(|character| character.is_whitespace())
        .count();
    let rest = &text[start..];
    let trailing = rest
        .iter()
        .rev()
        .take_while(|character| character.is_whitespace())
        .count();
    &rest[..rest.len() - trailing]
}

pub(crate) fn starts(text: &[char], index: usize, pattern: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    text.get(index..)
        .is_some_and(|rest| rest.starts_with(&pattern))
}

pub(crate) fn find(text: &[char], from: usize, pattern: &str) -> Option<usize> {
    (from..text.len()).find(|&at| starts(text, at, pattern))
}

/// The character index where the byte offset `byte` of `text` falls.
pub(crate) fn char_index(text: &str, byte: usize) -> usize {
    text[..byte].chars().count()
}

/// A regex match anchored at the start of `text`, as `(member, char end)`;
/// a pattern with no `member` group yields an empty member.
pub(crate) fn anchored(pattern: &Regex, text: &[char]) -> Option<(String, usize)> {
    let owned = string(text);
    let captures = pattern.captures(&owned)?;
    let member = match captures.name("member") {
        Some(found) => found.as_str().to_string(),
        None => String::new(),
    };
    Some((member, char_index(&owned, captures.get_match().end())))
}

/// What an error quotes of the manifest: the rest of the line it stopped on,
/// or that it stopped at the end of the manifest.
pub(crate) fn excerpt(text: &[char], index: usize) -> String {
    let line: String = match text.get(index..) {
        Some(rest) => rest
            .iter()
            .take_while(|character| **character != '\n')
            .collect(),
        None => String::new(),
    };
    match line.trim() {
        "" => "the end of the manifest".to_string(),
        line => line.to_string(),
    }
}

/// Whitespace and comments, of which this manifest has plenty inside literals.
pub(crate) fn skip_trivia(text: &[char], mut index: usize, origin: &str) -> Read<usize> {
    while index < text.len() {
        if text[index].is_whitespace() {
            index.tally();
        } else if starts(text, index, LINE_COMMENT) {
            // To the line's end; the newline itself is whitespace, stepped
            // over on the next turn.
            index = match find(text, index, "\n") {
                Some(end) => end,
                None => text.len(),
            };
        } else if starts(text, index, BLOCK_OPEN) {
            let end = find(text, index + BLOCK_OPEN.len(), BLOCK_CLOSE)
                .ok_or_else(|| format!("{origin}: a block comment is never closed"))?;
            index = end + BLOCK_CLOSE.len();
        } else {
            break;
        }
    }
    Ok(index)
}

/// Index just past the string literal starting at `index`.
pub(crate) fn skip_string(text: &[char], mut index: usize, origin: &str) -> Read<usize> {
    if starts(text, index, MULTILINE_QUOTE) {
        let end = find(text, index + MULTILINE_QUOTE.len(), MULTILINE_QUOTE)
            .ok_or_else(|| format!("{origin}: a multi-line string literal is never closed"))?;
        return Ok(end + MULTILINE_QUOTE.len());
    }
    index.tally();
    while index < text.len() {
        match text[index] {
            ESCAPE => {
                // The escape and the character it escapes.
                index.tally();
                index.tally();
            }
            QUOTE => {
                index.tally();
                return Ok(index);
            }
            '\n' => {
                return Err(format!(
                    "{origin}: a string literal runs off the end of its line"
                ))
            }
            _ => index.tally(),
        }
    }
    Err(format!("{origin}: a string literal is never closed"))
}

fn closer(opener: char) -> Option<char> {
    match opener {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

fn is_closer(character: char) -> bool {
    matches!(character, ')' | ']' | '}')
}

fn stray(text: &[char], index: usize, origin: &str) -> String {
    format!(
        "{origin}: '{}' closes nothing near {:?}",
        text[index],
        excerpt(text, index)
    )
}

/// Index just past the bracket group opening at `index`, brackets stacked.
pub(crate) fn balanced(text: &[char], mut index: usize, origin: &str) -> Read<usize> {
    let opened = index;
    let mut stack: Vec<char> = Vec::new();
    while index < text.len() {
        let character = text[index];
        if character == QUOTE {
            index = skip_string(text, index, origin)?;
        } else if starts(text, index, LINE_COMMENT) || starts(text, index, BLOCK_OPEN) {
            index = skip_trivia(text, index, origin)?;
        } else if let Some(expected) = closer(character) {
            stack.push(expected);
            index.tally();
        } else if is_closer(character) {
            if stack.pop() != Some(character) {
                return Err(stray(text, index, origin));
            }
            index.tally();
            if stack.is_empty() {
                return Ok(index);
            }
        } else {
            index.tally();
        }
    }
    Err(format!(
        "{origin}: a bracket opened at {:?} is never closed",
        excerpt(text, opened)
    ))
}

/// Comma-separated pieces at depth zero, ignoring commas inside anything.
pub(crate) fn split_top_level(text: &[char], origin: &str) -> Read<Vec<Vec<char>>> {
    let mut pieces = Vec::new();
    let mut start: usize = Default::default();
    let mut index = start;
    while index < text.len() {
        let character = text[index];
        if character == QUOTE {
            index = skip_string(text, index, origin)?;
        } else if starts(text, index, LINE_COMMENT) || starts(text, index, BLOCK_OPEN) {
            index = skip_trivia(text, index, origin)?;
        } else if closer(character).is_some() {
            index = balanced(text, index, origin)?;
        } else if is_closer(character) {
            return Err(stray(text, index, origin));
        } else if character == ',' {
            pieces.push(text[start..index].to_vec());
            index.tally();
            start = index;
        } else {
            index.tally();
        }
    }
    pieces.push(text[start..].to_vec());
    Ok(pieces
        .into_iter()
        .filter(|piece| !trimmed(piece).is_empty())
        .collect())
}
