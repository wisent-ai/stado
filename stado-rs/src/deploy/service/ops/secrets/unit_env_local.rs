//! `stado service unit-env-local`: the host half of `set_unit_env_key_on_host`,
//! run by the host's installed Stado as the unit's account. It sets or removes
//! one `Environment=` assignment in a systemd unit or drop-in without
//! following a symbolic link anywhere on the path, keeps the file's mode and
//! owner, writes a temporary copy and renames it only when the file did not
//! change underneath, and removes a drop-in left holding nothing but
//! `[Service]`. It prints `changed` or `unchanged`.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// One shell word as systemd reads an `Environment=` line: bare characters,
/// backslash escapes, and quoted runs.
static WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:[^\s"'\\]|\\.|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')+"#).expect("static")
});
static ASSIGNMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*Environment\s*=(.*)$").expect("static"));
static CONTINUATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\\r?\n").expect("static"));

fn fail(detail: impl std::fmt::Display) -> String {
    detail.to_string()
}

/// A shell word without its quoting, as `shlex.split` would give it.
fn unquote(word: &str) -> String {
    let mut out = String::new();
    let mut chars = word.chars();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), c) => out.push(c),
            (Some('"'), '\\') | (None, '\\') => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            (None, '\'' | '"') => quote = Some(c),
            (_, c) => out.push(c),
        }
    }
    out
}

fn target_path(raw: &str) -> Result<PathBuf, String> {
    match raw.strip_prefix("$HOME/") {
        Some(rest) => Ok(PathBuf::from(
            std::env::var("HOME").map_err(|_| fail("HOME is not set"))?,
        )
        .join(rest)),
        None => Ok(PathBuf::from(raw)),
    }
}

fn identity(meta: &fs::Metadata) -> (u64, u64, i64, i64, u64) {
    (
        meta.dev(),
        meta.ino(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.size(),
    )
}

/// The file's logical entries: a line ending in `\` continues into the next.
fn entries(text: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut pending = String::new();
    for line in text.split_inclusive('\n') {
        pending.push_str(line);
        if line.trim_end_matches(['\r', '\n']).ends_with('\\') {
            continue;
        }
        entries.push(std::mem::take(&mut pending));
    }
    if !pending.is_empty() {
        entries.push(pending);
    }
    entries
}

fn rewrite(original: &str, key: &str, value: Option<&str>) -> String {
    let mut output: Vec<String> = Vec::new();
    let (mut in_service, mut insertion) = (false, None);
    for raw in entries(original) {
        let logical = CONTINUATION.replace_all(&raw, " ").to_string();
        let stripped = logical.trim();
        if stripped.starts_with('[') && stripped.ends_with(']') {
            if in_service {
                insertion = Some(output.len());
            }
            in_service = stripped == "[Service]";
        }
        let assigned = in_service
            .then(|| ASSIGNMENT.captures(logical.trim_end_matches(['\r', '\n'])))
            .flatten()
            .map(|captures| captures[1].to_string())
            .filter(|body| !body.trim().is_empty());
        if let Some(body) = assigned {
            let tokens: Vec<&str> = WORDS.find_iter(&body).map(|m| m.as_str()).collect();
            let retained: Vec<&str> = tokens
                .iter()
                .copied()
                .filter(|token| unquote(token).split('=').next() != Some(key))
                .collect();
            if retained.len() != tokens.len() {
                if !retained.is_empty() {
                    output.push(format!("Environment={}\n", retained.join(" ")));
                }
                continue;
            }
        }
        output.push(raw);
    }
    if in_service {
        insertion = Some(output.len());
    }
    if let Some(value) = value {
        let at = match insertion {
            Some(at) => at,
            None => {
                output.push("\n[Service]\n".to_string());
                output.len()
            }
        };
        if at > 0 && !output[at - 1].ends_with('\n') {
            output[at - 1].push('\n');
        }
        let escaped = value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%");
        output.insert(at, format!("Environment=\"{key}={escaped}\"\n"));
    }
    output.concat()
}

fn update(
    raw_path: &str,
    key: &str,
    value: Option<&str>,
    uid: u32,
) -> Result<&'static str, String> {
    let path = target_path(raw_path)?;
    for component in path.ancestors() {
        if fs::symlink_metadata(component).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(fail("unit environment path cannot contain a symlink"));
        }
    }
    let before = fs::metadata(&path).map_err(fail)?;
    if !before.is_file() || before.uid() != uid {
        return Err(fail(
            "unit environment file must be regular and owned by the service account",
        ));
    }
    let original = fs::read_to_string(&path).map_err(fail)?;
    let updated = rewrite(&original, key, value);
    if updated == original {
        return Ok("unchanged");
    }
    let unchanged_since = |path: &Path| {
        fs::symlink_metadata(path).is_ok_and(|current| identity(&current) == identity(&before))
    };
    let empty_dropin = value.is_none()
        && path.extension().is_some_and(|ext| ext == "conf")
        && updated
            .lines()
            .all(|line| line.trim().is_empty() || line.trim() == "[Service]");
    if empty_dropin {
        if !unchanged_since(&path) {
            return Err(fail("unit environment file changed during the update"));
        }
        fs::remove_file(&path).map_err(fail)?;
        return Ok("changed");
    }
    let parent = path
        .parent()
        .ok_or_else(|| fail("unit environment file has no parent"))?;
    let temporary = parent.join(format!(".stado-unit-env.{}", std::process::id()));
    let written = (|| -> Result<(), String> {
        let mut stream = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(fail)?;
        stream
            .set_permissions(fs::Permissions::from_mode(before.mode() & 0o7777))
            .map_err(fail)?;
        std::os::unix::fs::fchown(&stream, Some(before.uid()), Some(before.gid())).map_err(fail)?;
        stream.write_all(updated.as_bytes()).map_err(fail)?;
        stream.sync_all().map_err(fail)?;
        if !unchanged_since(&path) {
            return Err(fail("unit environment file changed during the update"));
        }
        fs::rename(&temporary, &path).map_err(fail)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written.map(|()| "changed")
}

/// Print `changed` or `unchanged`; a refusal goes to stderr with exit 1.
pub fn unit_env_local(path_b64: &str, key_b64: &str, value_b64: Option<&str>, uid: u32) {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let decode = |text: &str| -> Result<String, String> {
        String::from_utf8(STANDARD.decode(text).map_err(fail)?).map_err(fail)
    };
    let result = (|| {
        let value = value_b64.map(decode).transpose()?;
        update(&decode(path_b64)?, &decode(key_b64)?, value.as_deref(), uid)
    })();
    match result {
        Ok(outcome) => println!("{outcome}"),
        Err(detail) => {
            eprintln!("{detail}");
            std::process::exit(1);
        }
    }
}
