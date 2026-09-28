//! Stage one, second half: the SSH argv shape shared by every remote
//! bootstrap command, and the two specs that install or re-qualify the
//! release binaries on the remote host.

use crate::deploy::{shlex_quote, CommandSpec};

use super::script::remote_install_script;

/// Python `_run_ssh` argv: `ssh -o StrictHostKeyChecking=accept-new TARGET CMD`.
pub fn ssh_argv(ssh_target: &str, command: &str) -> Vec<String> {
    vec![
        "ssh".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        ssh_target.to_string(),
        command.to_string(),
    ]
}

/// The release-binary download + path-resolution command.
pub fn install_spec(ssh_target: &str) -> CommandSpec {
    CommandSpec::new(ssh_argv(
        ssh_target,
        &remote_install_script(
            &crate::config::stado_api_url(),
            &crate::config::stado_release_version(),
        ),
    ))
}
/// Resolve an installed Stado binary when it is the registry's exact desired
/// version, or a newer version recorded by `release install-local`'s durable
/// handoff marker. The latter preserves a qualified candidate without trusting
/// an arbitrary executable that merely answers `--version`.
pub fn installed_spec(ssh_target: &str, expected_version: &str) -> CommandSpec {
    let script = format!(
        "set -eu\n\
         expected_version={}\n\
         stado_bin=\"$HOME/.stado/bin/stado\"\n\
         marker=\"$HOME/.stado/bin/stado.release-version\"\n\
         [ -x \"$stado_bin\" ] || {{ echo \"installed stado is missing\" >&2; exit 1; }}\n\
         set -- $(\"$stado_bin\" --version)\n\
         [ \"${{1:-}}\" = stado ] || {{ echo \"installed stado version is invalid\" >&2; exit 1; }}\n\
         actual_version=\"${{2:-}}\"\n\
         case \"$actual_version\" in\n\
           *[!0-9.]*|.*|*.|*..*) echo \"installed stado version is invalid\" >&2; exit 1 ;;\n\
         esac\n\
         IFS=. read -r a1 a2 a3 extra <<EOF\n\
$actual_version\n\
EOF\n\
         [ -n \"$a3\" ] && [ -z \"$extra\" ] || {{ echo \"installed stado version is invalid\" >&2; exit 1; }}\n\
         if [ \"$actual_version\" != \"$expected_version\" ]; then\n\
           marked_version=\"$(cat \"$marker\" 2>/dev/null || true)\"\n\
           [ \"$actual_version\" = \"$marked_version\" ] || {{ \
             echo \"installed stado candidate lacks its release marker\" >&2; exit 1; }}\n\
           IFS=. read -r e1 e2 e3 extra <<EOF\n\
$expected_version\n\
EOF\n\
           [ \"$a1\" -gt \"$e1\" ] \
             || {{ [ \"$a1\" -eq \"$e1\" ] && [ \"$a2\" -gt \"$e2\" ]; }} \
             || {{ [ \"$a1\" -eq \"$e1\" ] && [ \"$a2\" -eq \"$e2\" ] && [ \"$a3\" -gt \"$e3\" ]; }} \
             || {{ echo \"release-marked stado is not newer than registry desired\" >&2; exit 1; }}\n\
         fi\n\
         case \"$(uname -s)-$(uname -m)\" in\n\
           Linux-x86_64) platform=linux-amd64 ;;\n\
           Darwin-arm64) platform=darwin-arm64 ;;\n\
           *) echo \"unsupported platform: $(uname -s) $(uname -m)\" >&2; exit 1 ;;\n\
         esac\n\
         echo \"$platform\"\n\
         echo \"$stado_bin\"",
        shlex_quote(expected_version)
    );
    CommandSpec::new(ssh_argv(ssh_target, &script))
}
