//! The file text itself: a launchd plist in either domain, or a
//! `systemd --user` unit. One renderer per init system, and the plist's two
//! domains share theirs so an agent and its daemon spelling cannot drift.

use std::path::Path;
use std::borrow::Cow;
use std::fmt::Write;

/// Render a launchd agent plist with an explicit owner-controlled log path.
pub fn plist_text(
    label: &str,
    exec_args: &[String],
    env: &[(String, String)],
    log: &Path,
) -> String {
    plist_document(label, exec_args, env, log, None, Some("Aqua"))
}

/// The same job rendered for launchd's **system** domain, running as `user`.
///
/// The per-user domain does not exist on an ssh login with no Aqua session:
/// `launchctl bootstrap gui/$uid` answers `Could not switch to audit session`
/// and `stado service deploy` came back having installed nothing, which is how
/// two `stado agent` processes ran for four days with no unit behind them. A
/// daemon in `/Library/LaunchDaemons` is the domain that does exist over ssh,
/// and `UserName` is what keeps the process out of root: without it launchd
/// would run the fleet's own control binary as uid 0 against an account-owned
/// `~/.stado`.
pub fn daemon_plist_text(
    label: &str,
    exec_args: &[String],
    env: &[(String, String)],
    log: &Path,
    user: &str,
) -> String {
    plist_document(label, exec_args, env, log, Some(user), None)
}

/// One renderer for both domains, so the command, environment and logging of
/// an agent and its daemon spelling stay identical. The agent additionally
/// declares Aqua: a service deliberately placed in the GUI domain must not
/// silently load into a background bootstrap where browser work cannot open a
/// window.
fn plist_document(
    label: &str,
    exec_args: &[String],
    env: &[(String, String)],
    log: &Path,
    user: Option<&str>,
    session_type: Option<&str>,
) -> String {
    let user_xml = match user {
        Some(user) => format!("    <key>UserName</key>\n    <string>{}</string>\n", xml_text(user)),
        None => String::new(),
    };
    let session_xml = session_type
        .map(|session| {
            format!("    <key>LimitLoadToSessionType</key>\n    <string>{session}</string>\n")
        })
        .unwrap_or_default();
    let args_xml: String = exec_args
        .iter()
        .map(|a| format!("        <string>{}</string>\n", xml_text(a)))
        .collect();
    let env_xml: String = env
        .iter()
        .map(|(k, v)| format!("        <key>{}</key>\n        <string>{}</string>\n", xml_text(k), xml_text(v)))
        .collect();
    let log = log.to_string_lossy();
    let log = xml_text(&log);
    let label = xml_text(label);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
{args_xml}    </array>
{user_xml}{session_xml}    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <!-- launchd hands a job the system's soft `maxfiles`, which is 256 on
         macOS. The resolver holds transport connections for registry paths
         and a stream per in-flight adapter
         request; on 2026-09-02 it crossed that ceiling and every registry
         read for the next hours failed with `no registry SSH connection
         path answered (primary: Too many open files (os error 24))`, the
         job exited 1, launchd restarted it, and the cycle repeated. Release
         submits, promotions and `service directory show` failed at random
         inside that window. A resolver that reuses connections still needs
         more than 256 descriptors, so the unit says so rather than
         inheriting a desktop default. -->
    <key>SoftResourceLimits</key>
    <dict>
        <key>NumberOfFiles</key>
        <integer>4096</integer>
    </dict>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
    <key>EnvironmentVariables</key>
    <dict>
{env_xml}    </dict>
</dict>
</plist>
"#
    )
}

/// Python `_systemd_user_unit`.
pub fn systemd_user_unit(
    description: &str,
    exec_args: &[String],
    env: &[(String, String)],
) -> String {
    let env_lines: String = env
        .iter()
        .map(|(k, v)| format!("Environment={}\n", systemd_environment(k, v)))
        .collect();
    let cmd = systemd_command(exec_args);
    format!(
        // `LimitNOFILE` mirrors the plist's `SoftResourceLimits` above, for
        // the same reason and with the same number: a Linux member of this
        // fleet runs the same resolver against the same registry.
        "[Unit]\nDescription={description}\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nLimitNOFILE=4096\n{env_lines}ExecStart={cmd}\nRestart=on-failure\nRestartSec=30\n\n[Install]\nWantedBy=default.target\n"
    )
}

fn xml_text(value: &str) -> Cow<'_, str> {
    if !value.contains(['&', '<', '>', '"', '\'']) {
        return Cow::Borrowed(value);
    }
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    Cow::Owned(escaped)
}

/// Literal ExecStart argv, retaining empty arguments and argument boundaries.
pub(crate) fn systemd_command(arguments: &[String]) -> String {
    let mut command = String::new();
    for (index, argument) in arguments.iter().enumerate() {
        if index != 0 {
            command.push(' ');
        }
        append_systemd_word(&mut command, &[argument], true);
    }
    command
}

/// Environment values do not undergo ExecStart's dollar expansion.
pub(crate) fn systemd_environment(name: &str, value: &str) -> String {
    let mut result = String::new();
    append_systemd_word(&mut result, &[name, "=", value], false);
    result
}

fn append_systemd_word(result: &mut String, parts: &[&str], command: bool) {
    if command && parts.iter().any(|part| !part.is_empty())
        && parts.iter().all(|part| part.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':' | '=' | '@' | ',' | '+')
        }))
    {
        for part in parts {
            result.push_str(part);
        }
        return;
    }
    result.push('"');
    for part in parts {
        for ch in part.chars() {
            match ch {
                '\\' => result.push_str("\\\\"),
                '"' => result.push_str("\\\""),
                '%' => result.push_str("%%"),
                '$' if command => result.push_str("$$"),
                '\n' => result.push_str("\\n"),
                '\r' => result.push_str("\\r"),
                '\t' => result.push_str("\\t"),
                ch if ch.is_ascii_control() => {
                    write!(result, "\\x{:02x}", u32::from(ch)).expect("writing to a String");
                }
                _ => result.push(ch),
            }
        }
    }
    result.push('"');
}
