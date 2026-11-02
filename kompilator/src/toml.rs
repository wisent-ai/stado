//! The TOML a Cargo manifest and lock file are written in: tables, arrays of
//! tables, dotted keys, inline tables, arrays, basic and literal strings
//! (one-line and multi-line), integers, floats, booleans and dates kept as
//! text. Nothing else is needed to read what a build needs, and a document
//! this reader cannot read is refused with the line it stopped at.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Array(Vec<Value>),
    Table(BTreeMap<String, Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Boolean(value) => Some(*value),
            _ => None,
        }
    }
    pub fn as_table(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Table(table) => Some(table),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_table().and_then(|table| table.get(key))
    }
    /// The strings of an array value; none for anything else.
    pub fn strings(&self) -> Vec<String> {
        self.as_array().map(|items| items.iter().filter_map(|item| item.as_str().map(String::from)).collect()).unwrap_or_default()
    }
}

struct Reader<'a> {
    text: &'a [u8],
    at: usize,
}

fn fail<T>(reader: &Reader, what: &str) -> Result<T, String> {
    let line = reader.text[..reader.at.min(reader.text.len())].iter().filter(|byte| **byte == b'\n').count() + 1;
    Err(format!("line {line}: {what}"))
}

impl<'a> Reader<'a> {
    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }
    fn starts(&self, prefix: &str) -> bool {
        self.text[self.at..].starts_with(prefix.as_bytes())
    }
    /// Spaces and tabs.
    fn blank(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.at += 1;
        }
    }
    /// Spaces, tabs, line ends and comments.
    fn space(&mut self) {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\r' | b'\n') => self.at += 1,
                Some(b'#') => {
                    while !matches!(self.peek(), None | Some(b'\n')) {
                        self.at += 1;
                    }
                }
                _ => return,
            }
        }
    }
    /// The rest of a line after a value: a comment at most.
    fn end_of_line(&mut self) -> Result<(), String> {
        self.blank();
        if self.peek() == Some(b'#') {
            while !matches!(self.peek(), None | Some(b'\n')) {
                self.at += 1;
            }
        }
        match self.peek() {
            None | Some(b'\n') => Ok(()),
            Some(b'\r') if self.text.get(self.at + 1) == Some(&b'\n') => Ok(()),
            _ => fail(self, "text after a value"),
        }
    }

    fn key_part(&mut self) -> Result<String, String> {
        self.blank();
        match self.peek() {
            Some(b'"') => self.basic_string(),
            Some(b'\'') => self.literal_string(),
            _ => {
                let start = self.at;
                while matches!(self.peek(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-')) {
                    self.at += 1;
                }
                if start == self.at {
                    return fail(self, "a key was expected");
                }
                Ok(String::from_utf8_lossy(&self.text[start..self.at]).into_owned())
            }
        }
    }

    fn key(&mut self) -> Result<Vec<String>, String> {
        let mut parts = vec![self.key_part()?];
        loop {
            self.blank();
            if self.peek() == Some(b'.') {
                self.at += 1;
                parts.push(self.key_part()?);
            } else {
                return Ok(parts);
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<(), String> {
        let Some(code) = self.peek() else { return fail(self, "an escape at the end") };
        self.at += 1;
        match code {
            b'n' => out.push('\n'),
            b't' => out.push('\t'),
            b'r' => out.push('\r'),
            b'b' => out.push('\u{8}'),
            b'f' => out.push('\u{c}'),
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'u' | b'U' => {
                let width = if code == b'u' { 4 } else { 8 };
                let digits = std::str::from_utf8(&self.text[self.at..(self.at + width).min(self.text.len())])
                    .map_err(|_| String::from("an escape that is not text"))?;
                let value = u32::from_str_radix(digits, 16).map_err(|_| format!("the escape \\{} {digits}", code as char))?;
                out.push(char::from_u32(value).ok_or_else(|| format!("the escape {digits} names no character"))?);
                self.at += width;
            }
            b'\r' | b'\n' | b' ' | b'\t' => {
                // A line-ending backslash in a multi-line string trims to the next text.
                while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                    self.at += 1;
                }
            }
            other => return fail(self, &format!("the escape \\{}", other as char)),
        }
        Ok(())
    }

    fn basic_string(&mut self) -> Result<String, String> {
        if self.starts("\"\"\"") {
            self.at += 3;
            if self.peek() == Some(b'\n') {
                self.at += 1;
            } else if self.starts("\r\n") {
                self.at += 2;
            }
            let mut out = String::new();
            loop {
                if self.starts("\"\"\"") && !self.starts("\"\"\"\"") {
                    self.at += 3;
                    return Ok(out);
                }
                match self.peek() {
                    None => return fail(self, "a multi-line string never closed"),
                    Some(b'\\') => {
                        self.at += 1;
                        self.escape(&mut out)?;
                    }
                    Some(_) => self.char_into(&mut out),
                }
            }
        }
        self.at += 1;
        let mut out = String::new();
        loop {
            match self.peek() {
                None | Some(b'\n') => return fail(self, "a string never closed"),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    self.escape(&mut out)?;
                }
                Some(_) => self.char_into(&mut out),
            }
        }
    }

    fn literal_string(&mut self) -> Result<String, String> {
        if self.starts("'''") {
            self.at += 3;
            if self.peek() == Some(b'\n') {
                self.at += 1;
            } else if self.starts("\r\n") {
                self.at += 2;
            }
            let start = self.at;
            loop {
                if self.starts("'''") && !self.starts("''''") {
                    let text = String::from_utf8_lossy(&self.text[start..self.at]).into_owned();
                    self.at += 3;
                    return Ok(text);
                }
                if self.peek().is_none() {
                    return fail(self, "a multi-line literal string never closed");
                }
                self.at += 1;
            }
        }
        self.at += 1;
        let start = self.at;
        loop {
            match self.peek() {
                None | Some(b'\n') => return fail(self, "a literal string never closed"),
                Some(b'\'') => {
                    let text = String::from_utf8_lossy(&self.text[start..self.at]).into_owned();
                    self.at += 1;
                    return Ok(text);
                }
                Some(_) => self.at += 1,
            }
        }
    }

    /// One UTF-8 character copied whole.
    fn char_into(&mut self, out: &mut String) {
        let first = self.text[self.at];
        let width = match first {
            0x00..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            _ => 4,
        };
        let end = (self.at + width).min(self.text.len());
        out.push_str(&String::from_utf8_lossy(&self.text[self.at..end]));
        self.at = end;
    }

    fn value(&mut self) -> Result<Value, String> {
        self.blank();
        match self.peek() {
            Some(b'"') => Ok(Value::String(self.basic_string()?)),
            Some(b'\'') => Ok(Value::String(self.literal_string()?)),
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                loop {
                    self.space();
                    if self.peek() == Some(b']') {
                        self.at += 1;
                        return Ok(Value::Array(items));
                    }
                    items.push(self.value()?);
                    self.space();
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b']') => {}
                        _ => return fail(self, "an array item is followed by neither , nor ]"),
                    }
                }
            }
            Some(b'{') => {
                self.at += 1;
                let mut table = BTreeMap::new();
                loop {
                    self.blank();
                    if self.peek() == Some(b'}') {
                        self.at += 1;
                        return Ok(Value::Table(table));
                    }
                    let key = self.key()?;
                    self.blank();
                    if self.peek() != Some(b'=') {
                        return fail(self, "an inline table key has no =");
                    }
                    self.at += 1;
                    let value = self.value()?;
                    insert(&mut table, &key, value).map_err(|why| format!("in an inline table: {why}"))?;
                    self.blank();
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {}
                        _ => return fail(self, "an inline table entry is followed by neither , nor }"),
                    }
                }
            }
            Some(b't') if self.starts("true") => {
                self.at += 4;
                Ok(Value::Boolean(true))
            }
            Some(b'f') if self.starts("false") => {
                self.at += 5;
                Ok(Value::Boolean(false))
            }
            Some(_) => {
                let start = self.at;
                while !matches!(self.peek(), None | Some(b',' | b']' | b'}' | b'\n' | b'\r' | b'#')) {
                    self.at += 1;
                }
                let word = String::from_utf8_lossy(&self.text[start..self.at]).trim().replace('_', "");
                if let Ok(integer) = word.parse::<i64>() {
                    return Ok(Value::Integer(integer));
                }
                if let Ok(float) = word.parse::<f64>() {
                    return Ok(Value::Float(float));
                }
                if word.is_empty() {
                    return fail(self, "a value was expected");
                }
                // Dates and times are kept as their text.
                Ok(Value::String(word))
            }
            None => fail(self, "a value was expected at the end"),
        }
    }
}

/// Set `value` at the dotted `key` under `table`, creating tables between.
fn insert(table: &mut BTreeMap<String, Value>, key: &[String], value: Value) -> Result<(), String> {
    let (last, path) = key.split_last().ok_or("an empty key")?;
    let mut here = table;
    for part in path {
        let entry = here.entry(part.clone()).or_insert_with(|| Value::Table(BTreeMap::new()));
        here = match entry {
            Value::Table(next) => next,
            _ => return Err(format!("{part} is not a table")),
        };
    }
    if here.contains_key(last) {
        return Err(format!("the key {} is defined twice", key.join(".")));
    }
    here.insert(last.clone(), value);
    Ok(())
}

/// The table at `path` under `root`, created when absent; with `array`, a new
/// table appended to the array of tables at `path`.
fn open<'t>(root: &'t mut BTreeMap<String, Value>, path: &[String], array: bool) -> Result<&'t mut BTreeMap<String, Value>, String> {
    let mut here = root;
    for (position, part) in path.iter().enumerate() {
        let last = position + 1 == path.len();
        if last && array {
            let entry = here.entry(part.clone()).or_insert_with(|| Value::Array(Vec::new()));
            let Value::Array(items) = entry else { return Err(format!("{part} is not an array of tables")) };
            items.push(Value::Table(BTreeMap::new()));
            let Some(Value::Table(table)) = items.last_mut() else { return Err(String::from("unreachable")) };
            return Ok(table);
        }
        let entry = here.entry(part.clone()).or_insert_with(|| Value::Table(BTreeMap::new()));
        here = match entry {
            Value::Table(next) => next,
            Value::Array(items) => match items.last_mut() {
                Some(Value::Table(table)) => table,
                _ => return Err(format!("{part} is an array that holds no table")),
            },
            _ => return Err(format!("{part} is not a table")),
        };
    }
    Ok(here)
}

/// Read a whole TOML document.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut reader = Reader { text: text.as_bytes(), at: 0 };
    let mut root = BTreeMap::new();
    let mut current: Vec<String> = Vec::new();
    loop {
        reader.space();
        let Some(byte) = reader.peek() else { break };
        if byte == b'[' {
            let array = reader.starts("[[");
            reader.at += if array { 2 } else { 1 };
            let path = reader.key()?;
            reader.blank();
            let close = if array { "]]" } else { "]" };
            if !reader.starts(close) {
                return fail(&reader, "a table header never closed");
            }
            reader.at += close.len();
            reader.end_of_line()?;
            open(&mut root, &path, array).map_err(|why| format!("table [{}]: {why}", path.join(".")))?;
            current = path;
            if array {
                // Later keys go into the table just appended.
                current.push(String::from("\u{0}last"));
            }
            continue;
        }
        let key = reader.key()?;
        reader.blank();
        if reader.peek() != Some(b'=') {
            return fail(&reader, "a key has no =");
        }
        reader.at += 1;
        let value = reader.value()?;
        reader.end_of_line()?;
        let path: Vec<String> = current.iter().filter(|part| part.as_str() != "\u{0}last").cloned().collect();
        let table = open(&mut root, &path, false).map_err(|why| format!("table [{}]: {why}", path.join(".")))?;
        insert(table, &key, value).map_err(|why| {
            let line = reader.text[..reader.at].iter().filter(|byte| **byte == b'\n').count() + 1;
            format!("line {line}: {why}")
        })?;
    }
    Ok(Value::Table(root))
}
