/// The environment a `systemd --user` unit declares.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SystemdUnit {
    pub env: Vec<(String, String)>,
    pub environment_files: Vec<String>,
}

/// Read `[Service]`'s `Environment=` and `EnvironmentFile=` directives.
///
/// Follows systemd's own rules for the cases that change the answer:
/// backslash line continuations are joined, a bare `Environment=` clears
/// everything set before it, and one directive may carry several
/// quoted assignments.
pub fn parse_systemd_unit(text: &str) -> SystemdUnit {
    let mut parsed = SystemdUnit::default();
    let mut section = String::new();
    for line in logical_lines(text) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = name.trim().to_string();
            continue;
        }
        if section != "Service" {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Environment" if value.trim().is_empty() => parsed.env.clear(),
            "Environment" => {
                for word in split_words(value) {
                    if let Some((name, setting)) = word.split_once('=') {
                        parsed.env.push((name.to_string(), setting.to_string()));
                    }
                }
            }
            "EnvironmentFile" if value.trim().is_empty() => parsed.environment_files.clear(),
            "EnvironmentFile" => parsed.environment_files.push(value.trim().to_string()),
            _ => {}
        }
    }
    parsed
}

/// Join backslash line continuations into logical directives.
pub(crate) fn logical_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut pending = String::new();
    for raw in text.lines() {
        let trimmed = raw.trim_end();
        if let Some(head) = trimmed.strip_suffix('\\') {
            pending.push_str(head.trim_end());
            pending.push(' ');
            continue;
        }
        pending.push_str(trimmed);
        lines.push(std::mem::take(&mut pending));
    }
    if !pending.is_empty() {
        lines.push(pending);
    }
    lines
}

/// Split a directive value into whitespace-separated words, honouring both
/// quote characters, so a quoted assignment whose value contains a space
/// stays one assignment instead of splitting into two words.
pub(crate) fn split_words(value: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for ch in value.chars() {
        match quote {
            Some(open) if ch == open => quote = None,
            Some(_) => current.push(ch),
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                started = true;
            }
            None if ch.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            None => {
                current.push(ch);
                started = true;
            }
        }
    }
    if started {
        words.push(current);
    }
    words
}
