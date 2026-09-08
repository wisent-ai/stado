//! The remote programs for the entries that stand in the managed account's
//! own home before they run.

use crate::deploy::shlex_quote;

/// The remote script for a read inside the managed account's home: stand in
/// that home, then become the entry's own fixed argv.
///
/// Every word is a compile-time constant of this module and is quoted for the
/// remote shell anyway. The operator's words selected the entry and reach the
/// host in nothing else, so barrier three holds exactly as it does on the
/// [`crate::deploy::host_channel::run_program`] path.
pub fn home_rooted_script(argv: &[&str]) -> String {
    let fixed = argv
        .iter()
        .map(|word| shlex_quote(word))
        .collect::<Vec<String>>()
        .join(" ");
    format!("set -eu\ncd \"$HOME\"\nexec {fixed}\n")
}

/// The fixed mutating entry's real program. `mkdir -p` alone inherits an
/// ambient umask and follows symlinked parents; this script supplies the
/// restrictive mode and refuses every existing symlink or foreign owner.
pub fn probierz_run_root_script() -> String {
    let mut script = String::from("set -eu\numask 077\ncd \"$HOME\"\n");
    for path in [".stado", ".stado/work", ".stado/work/runs"] {
        let quoted = shlex_quote(path);
        script.push_str(&format!(
            "[ ! -L {quoted} ] || {{ printf '%s\\n' {}; exit 1; }}\n",
            shlex_quote(&format!(
                "refusing run-directory creation: managed path traverses a symlink at $HOME/{path}"
            ))
        ));
        script.push_str(&format!(
            "if [ -e {quoted} ]; then [ -d {quoted} ] || {{ printf '%s\\n' {}; exit 1; }}; [ -O {quoted} ] || {{ printf '%s\\n' {}; exit 1; }}; else /bin/mkdir {quoted}; /bin/chmod 700 {quoted}; fi\n",
            shlex_quote(&format!(
                "refusing run-directory creation: managed path is not a directory at $HOME/{path}"
            )),
            shlex_quote(&format!(
                "refusing run-directory creation: managed path is not owned by this account at $HOME/{path}"
            )),
        ));
    }
    script.push_str("/bin/chmod 700 .stado/work/runs\nprintf '%s\\n' \"$HOME/.stado/work/runs\"\n");
    script
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::host_exec::allowlist::{approve, PROBIERZ_RUN_ROOT_CREATE};

    #[test]
    fn run_root_preparation_is_one_fixed_guarded_mutation() {
        let selected = approve(&["mkdir".into(), "-p".into(), ".stado/work/runs".into()])
            .expect("fixed run root is approved");
        assert_eq!(selected.argv, PROBIERZ_RUN_ROOT_CREATE);
        let script = probierz_run_root_script();
        assert!(script.contains("umask 077"));
        assert!(script.contains("[ ! -L .stado/work/runs ]"));
        assert!(script.contains("/bin/chmod 700 .stado/work/runs"));
        assert!(!script.contains("$1"));
    }
}
