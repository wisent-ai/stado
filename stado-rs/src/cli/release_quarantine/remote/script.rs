//! The remote read program, and the three bodies that decide how much of a
//! file it sends back.

/// Read one file on a registry host. Absent and empty are different answers:
/// `Ok(None)` is "there is no such file", `Ok(Some(""))` is "the file is there
/// and has nothing in it", and an operator draws opposite conclusions from
/// those two. The content comes back base64 so a line of the file cannot forge
/// one of the markers that frame it.
pub(super) const READ_TEMPLATE: &str = r#"set -eu
path=@PATH@
if [ ! -f "$path" ]; then
  printf 'STADO_QUARANTINE_ABSENT\t%s\n' "$path"
  exit 0
fi
printf 'STADO_QUARANTINE_BYTES\t%s\n' "$(/usr/bin/wc -c < "$path" | /usr/bin/tr -d ' ')"
printf 'STADO_QUARANTINE_BASE64\t%s\n' "$(@BODY@ | /usr/bin/openssl base64 -A)"
"#;

pub(super) const READ_WHOLE_BODY: &str = r#"/usr/bin/head -c @LIMIT@ "$path""#;
pub(super) const READ_HEAD_BODY: &str =
    r#"/usr/bin/head -n @LINES@ "$path" | /usr/bin/head -c @LIMIT@"#;

pub(super) const READ_TAIL_BODY: &str =
    r#"/usr/bin/tail -n @LINES@ "$path" | /usr/bin/head -c @LIMIT@"#;
