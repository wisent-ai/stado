//! A Swift literal in the plist subset: strings, numbers, booleans, arrays and
//! dictionaries, read into the same `plist::Value` an Info.plist file parses
//! to. Anything richer — an interpolation, an identifier, an expression — is
//! refused rather than guessed at, because a value this reader cannot read is
//! unknown, and an unknown value must not quietly become an absent name.

use plist::{Dictionary, Value};

use super::{excerpt, skip_string, skip_trivia, starts, string, Read};

const STEP: usize = 1;
/// Where a literal's cursor starts: the first character of its text.
const TEXT_START: usize = 0;
const QUOTE: char = '"';
const ESCAPE: char = '\\';
const MULTILINE_QUOTE: &str = "\"\"\"";
const INTERPOLATION: &str = "\\(";

fn identifier_start(character: char) -> bool {
    character.is_ascii_alphabetic() || character == '_'
}

fn identifier_part(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn escaped(character: char) -> Option<char> {
    match character {
        QUOTE => Some(QUOTE),
        ESCAPE => Some(ESCAPE),
        'n' => Some('\n'),
        't' => Some('\t'),
        'r' => Some('\r'),
        '0' => Some('\0'),
        _ => None,
    }
}

pub(crate) struct Literal<'a> {
    text: &'a [char],
    origin: &'a str,
    index: usize,
}

impl<'a> Literal<'a> {
    pub(crate) fn new(text: &'a [char], origin: &'a str) -> Self {
        Self {
            text,
            origin,
            index: TEXT_START,
        }
    }

    /// The one literal `text` holds, with nothing after it.
    pub(crate) fn parse(mut self) -> Read<Value> {
        let value = self.value()?;
        self.index = skip_trivia(self.text, self.index, self.origin)?;
        if self.index != self.text.len() {
            return Err(format!(
                "{}: trailing text after a literal: {:?}",
                self.origin,
                excerpt(self.text, self.index)
            ));
        }
        Ok(value)
    }

    fn at(&self, pattern: &str) -> bool {
        starts(self.text, self.index, pattern)
    }

    fn value(&mut self) -> Read<Value> {
        self.index = skip_trivia(self.text, self.index, self.origin)?;
        let Some(&character) = self.text.get(self.index) else {
            return Err(format!("{}: a value is missing", self.origin));
        };
        if character == QUOTE {
            return self.string().map(Value::String);
        }
        if character == '[' {
            return self.collection();
        }
        if let Some(word) = self.word() {
            if word == "true" || word == "false" {
                self.index += word.len();
                return Ok(Value::Boolean(word == "true"));
            }
        }
        if let Some(end) = self.number() {
            let number = string(&self.text[self.index..end]);
            self.index = end;
            return Ok(Value::String(number));
        }
        Err(format!(
            "{}: {:?} is not a literal this reader understands, so the value is unknown rather than absent",
            self.origin,
            excerpt(self.text, self.index)
        ))
    }

    fn word(&self) -> Option<String> {
        let first = *self.text.get(self.index)?;
        if !identifier_start(first) {
            return None;
        }
        let length = self.text[self.index..]
            .iter()
            .take_while(|&&character| identifier_part(character))
            .count();
        Some(string(&self.text[self.index..self.index + length]))
    }

    /// End of `-?[0-9][0-9_]*(\.[0-9]+)?` at the cursor.
    fn number(&self) -> Option<usize> {
        let digit = |at: usize| self.text.get(at).is_some_and(char::is_ascii_digit);
        let mut at = self.index;
        if self.text.get(at) == Some(&'-') {
            at += STEP;
        }
        if !digit(at) {
            return None;
        }
        while digit(at) || self.text.get(at) == Some(&'_') {
            at += STEP;
        }
        if self.text.get(at) == Some(&'.') && digit(at + STEP) {
            at += STEP;
            while digit(at) {
                at += STEP;
            }
        }
        Some(at)
    }

    fn string(&mut self) -> Read<String> {
        let end = skip_string(self.text, self.index, self.origin)?;
        let raw = &self.text[self.index..end];
        self.index = end;
        let quoted = string(raw);
        if starts(raw, TEXT_START, MULTILINE_QUOTE) {
            let inner = &raw[MULTILINE_QUOTE.len()..raw.len() - MULTILINE_QUOTE.len()];
            return Ok(string(inner));
        }
        if quoted.contains(INTERPOLATION) {
            return Err(format!(
                "{}: {quoted:?} interpolates, so its value cannot be read without running the manifest",
                self.origin
            ));
        }
        let mut body = raw[STEP..raw.len() - STEP].iter();
        let mut out = String::new();
        while let Some(&character) = body.next() {
            if character != ESCAPE {
                out.push(character);
                continue;
            }
            let Some(&next) = body.next() else {
                return Err(format!("{}: {quoted:?} ends in an escape", self.origin));
            };
            let Some(replacement) = escaped(next) else {
                return Err(format!(
                    "{}: unknown escape '{ESCAPE}{next}' in {quoted:?}",
                    self.origin
                ));
            };
            out.push(replacement);
        }
        Ok(out)
    }

    fn separator(&mut self) -> Read<()> {
        self.index = skip_trivia(self.text, self.index, self.origin)?;
        Ok(())
    }

    fn mixed(&self) -> String {
        format!(
            "{}: a literal mixes array and dictionary entries",
            self.origin
        )
    }

    fn collection(&mut self) -> Read<Value> {
        self.index += STEP;
        self.separator()?;
        if self.at(":]") {
            self.index += ":]".len();
            return Ok(Value::Dictionary(Dictionary::new()));
        }
        if self.at("]") {
            self.index += STEP;
            return Ok(Value::Array(Vec::new()));
        }
        let mut items = Vec::new();
        let mut pairs = Dictionary::new();
        let mut keyed: Option<bool> = None;
        loop {
            let first = self.value()?;
            self.separator()?;
            if self.at(":") {
                if keyed == Some(false) {
                    return Err(self.mixed());
                }
                keyed = Some(true);
                self.index += STEP;
                let Value::String(key) = first else {
                    return Err(format!("{}: a dictionary key is not a string", self.origin));
                };
                if pairs.contains_key(&key) {
                    return Err(format!("{}: duplicate key {key:?}", self.origin));
                }
                let value = self.value()?;
                pairs.insert(key, value);
            } else {
                if keyed == Some(true) {
                    return Err(self.mixed());
                }
                keyed = Some(false);
                items.push(first);
            }
            self.separator()?;
            if self.at(",") {
                self.index += STEP;
                self.separator()?;
                if self.at("]") {
                    self.index += STEP;
                    break;
                }
                continue;
            }
            if self.at("]") {
                self.index += STEP;
                break;
            }
            return Err(format!(
                "{}: expected ',' or ']' at {:?}",
                self.origin,
                excerpt(self.text, self.index)
            ));
        }
        Ok(if keyed == Some(true) {
            Value::Dictionary(pairs)
        } else {
            Value::Array(items)
        })
    }
}

/// The non-empty string literal `text` holds, trimmed.
pub(crate) fn string_argument(text: &[char], origin: &str, what: &str) -> Read<String> {
    match Literal::new(text, origin).parse()? {
        Value::String(value) if !value.trim().is_empty() => Ok(value.trim().to_string()),
        _ => Err(format!(
            "{origin}: {what} is not a non-empty string literal"
        )),
    }
}
