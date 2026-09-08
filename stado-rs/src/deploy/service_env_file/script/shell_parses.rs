//! What defends the delivered program: every rendering of it has to parse as
//! shell, checked by the shell itself.

use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    /// The remote program must be valid shell, checked by the real shell.
    ///
    /// This exists because of a mistake worth making impossible: the awk
    /// program is delivered inside a single-quoted shell word, and one
    /// apostrophe in an awk COMMENT ends that word and truncates the program.
    /// The host then reported `unexpected EOF while looking for matching "` and
    /// every command in this module failed at once. `bash -n` is the only
    /// reviewer that cannot miss it.
    #[test]
    fn every_rendered_script_parses_as_shell() {
        for request in [
            EnvFileRequest::read("$HOME/.config/weles/worker.env"),
            EnvFileRequest {
                env_path: "$HOME/.config/weles/worker.env",
                reveal: Some("WELES_API_TOKEN"),
                expect: Some(("WC_SKARBIEC_URL", "http://127.0.0.1:8895")),
            },
        ] {
            let script = remote_env_file_script(&request);
            let mut child = std::process::Command::new("bash")
                .arg("-n")
                .stdin(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("bash runs");
            std::io::Write::write_all(
                child.stdin.as_mut().expect("bash takes stdin"),
                script.as_bytes(),
            )
            .expect("script is written");
            drop(child.stdin.take());
            let output = child.wait_with_output().expect("bash finishes");
            assert!(
                output.status.success(),
                "the rendered script is not valid shell: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
