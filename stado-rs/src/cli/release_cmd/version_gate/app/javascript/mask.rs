//! The one masking pass every JavaScript reader goes through: comments and
//! the bodies of string, template and regular-expression literals blanked,
//! newlines and byte offsets kept, each string or template body returned
//! with its offset. A file it cannot read with certainty is refused.

use super::super::surface::{Loader, Read};

/// A character after which `/` opens a regular expression, not a division.
const REGEX_PRECEDERS: &[u8] = b"=(,:[!&|?+-*%~^<>{};";
/// The characters that may follow a regular expression's closing `/`.
const REGEX_FLAGS: &[u8] = b"dgimsuy";
/// A backslash escapes the byte after it, so both are skipped together.
const ESCAPE_WIDTH: usize = 2;
/// The code with every comment and literal body blanked (newlines kept, byte
/// offsets unchanged), and each string or template body with its offset.
pub(super) struct Masked {
    pub(super) code: String,
    pub(super) literals: Vec<(usize, String)>,
}

fn blank(bytes: &[u8]) -> impl Iterator<Item = u8> + '_ {
    bytes
        .iter()
        .map(|byte| if *byte == b'\n' { b'\n' } else { b' ' })
}

/// Where the literal opened at `start` by `quote` ends (exclusive).
fn literal_end(text: &[u8], start: usize, quote: u8) -> Option<usize> {
    let mut index = start + 1;
    while index < text.len() {
        match text[index] {
            b'\\' => index += ESCAPE_WIDTH,
            b'\n' if quote != b'`' => return None,
            byte if byte == quote => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

/// Where a regular expression opened at `start` ends, flags included.
fn regex_end(text: &[u8], start: usize) -> Option<usize> {
    let (mut index, mut in_class) = (start + 1, false);
    while index < text.len() {
        match text[index] {
            b'\\' => index += ESCAPE_WIDTH,
            b'\n' => return None,
            b'[' => (in_class, index) = (true, index + 1),
            b']' => (in_class, index) = (false, index + 1),
            b'/' if !in_class => {
                index += 1;
                while index < text.len() && REGEX_FLAGS.contains(&text[index]) {
                    index += 1;
                }
                return Some(index);
            }
            _ => index += 1,
        }
    }
    None
}

fn unclosed_substitution(inner: &str) -> bool {
    let (mut depth, mut previous) = (0usize, ' ');
    for character in inner.chars() {
        if character == '{' && previous == '$' {
            depth += 1;
        } else if character == '}' && depth > 0 {
            depth -= 1;
        }
        previous = character;
    }
    depth > 0
}

/// Whether a `/` at this point of `code` opens a regular expression.
fn opens_regex(code: &[u8]) -> bool {
    code.iter()
        .rev()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| REGEX_PRECEDERS.contains(byte))
}

fn mask(path: &str, source: &str) -> Read<Masked> {
    let text = source.as_bytes();
    let mut code = Vec::with_capacity(text.len());
    let mut literals = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let byte = text[index];
        let end = match (byte, text.get(index + 1).copied()) {
            (b'/', Some(b'/')) => text[index..]
                .iter()
                .position(|found| *found == b'\n')
                .map_or(text.len(), |at| index + at),
            (b'/', Some(b'*')) => source[index..]
                .find("*/")
                .map(|at| index + at + "*/".len())
                .ok_or_else(|| {
                    format!("{path}: a block comment at byte {index} is never closed")
                })?,
            (b'"' | b'\'' | b'`', _) => {
                let end = literal_end(text, index, byte)
                    .ok_or_else(|| format!("{path}: a string at byte {index} is never closed"))?;
                let inner = &source[index + 1..end - 1];
                if byte == b'`' && unclosed_substitution(inner) {
                    return Err(format!(
                        "{path}: the template at byte {index} has an unclosed ${{...}}; \
                         refusing to guess where it ends"
                    ));
                }
                literals.push((index, inner.to_string()));
                code.push(byte);
                code.extend(blank(inner.as_bytes()));
                code.push(byte);
                index = end;
                continue;
            }
            (b'/', _) if opens_regex(&code) => regex_end(text, index).ok_or_else(|| {
                format!("{path}: a regular expression at byte {index} is never closed")
            })?,
            _ => {
                code.push(byte);
                index += 1;
                continue;
            }
        };
        code.extend(blank(&text[index..end]));
        index = end;
    }
    let code = String::from_utf8(code).map_err(|error| format!("{path}: {error}"))?;
    for (opener, closer) in [('{', '}'), ('[', ']')] {
        if code.matches(opener).count() != code.matches(closer).count() {
            return Err(format!(
                "{path}: {opener}{closer} are unbalanced after masking; refusing to report a surface"
            ));
        }
    }
    Ok(Masked { code, literals })
}

pub(super) fn masked(load: Loader, path: &str) -> Read<Masked> {
    let source =
        String::from_utf8(load(path)?).map_err(|error| format!("{path}: not UTF-8 ({error})"))?;
    mask(path, &source)
}
