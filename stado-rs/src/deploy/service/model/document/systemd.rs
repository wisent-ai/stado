use crate::deploy::DeployError;

/// The environment a `systemd --user` unit declares.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SystemdUnit {
    pub env: Vec<(String, String)>,
    pub environment_files: Vec<String>,
    pub exec_start: Vec<Vec<String>>,
    /// Directive locations still requiring systemd's runtime substitution.
    /// A migration must resolve these before treating the values as literals.
    pub unresolved_expansions: Vec<String>,
}

/// Read `[Service]` environment declarations and effective `ExecStart` vectors.
///
/// Follows systemd's own rules for the cases that change the answer:
/// backslash line continuations are joined, a bare `Environment=` clears
/// everything set before it, and one directive may carry several
/// quoted assignments.
pub fn parse_systemd_unit(text: &str) -> Result<SystemdUnit, DeployError> {
    let mut parsed = SystemdUnit::default();
    let mut section = String::new();
    let mut unresolved_environment = std::collections::BTreeSet::new();
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
            "Environment" if value.trim().is_empty() => {
                parsed.env.clear();
                unresolved_environment.clear();
            }
            "Environment" => {
                for mut word in split_words(value)? {
                    let unresolved = decode_literals(&mut word, false);
                    if let Some((name, setting)) = word.split_once('=') {
                        if unresolved {
                            unresolved_environment.insert(name.to_string());
                        } else {
                            unresolved_environment.remove(name);
                        }
                        parsed.env.push((name.to_string(), setting.to_string()));
                    }
                }
            }
            "EnvironmentFile" if value.trim().is_empty() => parsed.environment_files.clear(),
            "EnvironmentFile" => parsed.environment_files.push(value.trim().to_string()),
            "ExecStart" if value.trim().is_empty() => {
                parsed.exec_start.clear();
                parsed.unresolved_expansions.clear();
            }
            "ExecStart" => {
                let (arguments, unresolved) = systemd_arguments(value)?;
                let command_index = parsed.exec_start.len();
                parsed.unresolved_expansions.extend(
                    unresolved
                        .into_iter()
                        .map(|index| format!("ExecStart[{command_index}] argument[{index}]")),
                );
                parsed.exec_start.push(arguments);
            }
            _ => {}
        }
    }
    parsed.unresolved_expansions.extend(
        unresolved_environment
            .into_iter()
            .map(|name| format!("Environment:{name}")),
    );
    Ok(parsed)
}

/// Decode literal %% and, only for ExecStart, $$. Single introducers remain
/// visible and prevent a migration from accidentally quoting a substitution
/// into a literal. Retain removes only the repeated ASCII character in place.
fn decode_literals(value: &mut String, command: bool) -> bool {
    let mut pending = None;
    let mut unresolved = false;
    value.retain(|ch| {
        if let Some(previous) = pending.take() {
            if ch == previous {
                return false;
            }
            unresolved = true;
        }
        if ch == '%' || (command && ch == '$') {
            pending = Some(ch);
        }
        true
    });
    unresolved || pending.is_some()
}

/// Decode the same native argument spelling for both a captured ExecStart and
/// the declaration compared with it. The indices retain unresolved substitutions.
pub(crate) fn systemd_arguments(value: &str) -> Result<(Vec<String>, Vec<usize>), DeployError> {
    let mut arguments = split_words(value)?;
    let mut unresolved = Vec::new();
    for (index, argument) in arguments.iter_mut().enumerate() {
        if decode_literals(argument, true) {
            unresolved.push(index);
        }
    }
    Ok((arguments, unresolved))
}

/// Join backslash line continuations into logical directives.
pub(crate) fn logical_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut pending = String::new();
    for raw in text.lines() {
        let trimmed = raw.trim_end();
        let content = trimmed.trim_start();
        if !pending.is_empty()
            && (content.is_empty() || content.starts_with('#') || content.starts_with(';'))
        {
            continue;
        }
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
pub(crate) fn split_words(value: &str) -> Result<Vec<String>, DeployError> {
    let mut words = Vec::new();
    let mut current = Vec::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            unescape(&mut chars, &mut current)?;
            started = true;
            continue;
        }
        match quote {
            Some(open) if ch == open => quote = None,
            Some(_) => current.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes()),
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                started = true;
            }
            None if ch.is_ascii_whitespace() => {
                if started {
                    words.push(word(std::mem::take(&mut current))?);
                    started = false;
                }
            }
            None => {
                current.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err(DeployError(
            "systemd directive has an unterminated quote".to_string(),
        ));
    }
    if started {
        words.push(word(current)?);
    }
    Ok(words)
}

fn word(bytes: Vec<u8>) -> Result<String, DeployError> {
    if bytes.contains(&0) {
        return Err(DeployError(
            "systemd directive contains a NUL byte".to_string(),
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| DeployError("systemd directive decodes to non-UTF-8 bytes".to_string()))
}

fn unescape(chars: &mut std::str::Chars<'_>, output: &mut Vec<u8>) -> Result<(), DeployError> {
    let invalid = || DeployError("systemd directive has an invalid escape sequence".to_string());
    let first = chars.next().ok_or_else(invalid)?;
    let (radix, digits, mut code, unicode) = match first {
        'a' => (0, 0, 7, false),
        'b' => (0, 0, 8, false),
        'f' => (0, 0, 12, false),
        'n' => (0, 0, 10, false),
        'r' => (0, 0, 13, false),
        't' => (0, 0, 9, false),
        'v' => (0, 0, 11, false),
        's' => (0, 0, u32::from(b' '), false),
        '\\' | '"' | '\'' => (0, 0, u32::from(first), false),
        'x' => (16, 2, 0, false),
        'u' => (16, 4, 0, true),
        'U' => (16, 8, 0, true),
        '0'..='7' => (8, 2, first.to_digit(8).ok_or_else(invalid)?, false),
        _ => return Err(invalid()),
    };
    for _ in 0..digits {
        let digit = chars
            .next()
            .and_then(|ch| ch.to_digit(radix))
            .ok_or_else(invalid)?;
        code = code
            .checked_mul(radix)
            .and_then(|code| code.checked_add(digit))
            .ok_or_else(invalid)?;
    }
    if code == 0 {
        return Err(invalid());
    }
    if unicode {
        let ch = char::from_u32(code).ok_or_else(invalid)?;
        output.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
    } else {
        output.push(u8::try_from(code).map_err(|_| invalid())?);
    }
    Ok(())
}
