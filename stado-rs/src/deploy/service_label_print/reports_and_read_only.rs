//! What defends the answer: a report says loaded only when a domain held the
//! label, the bare program is used only when there is no argv, an unnamed
//! environment line is never echoed, and the delivered program only reads.

use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loaded_system_job_reports_its_pid_and_argv() {
        let stdout = "STADO_LABEL_DOMAIN\tsystem\n\
             STADO_LABEL_FIELD\tpid\t57572\n\
             STADO_LABEL_FIELD\tstate\trunning\n\
             STADO_LABEL_FIELD\tpath\t/Library/LaunchDaemons/x.plist\n\
             STADO_LABEL_FIELD\targuments\t/u/.stado/bin/stado agent --target mini\n\
             STADO_LABEL_DONE\tyes\n";
        let state = parse_label_print("mini", "x", stdout);
        assert!(state.loaded());
        assert_eq!(state.domain.as_deref(), Some("system"));
        assert_eq!(state.pid.as_deref(), Some("57572"));
        assert_eq!(
            state.runs(),
            Some("/u/.stado/bin/stado agent --target mini")
        );
    }

    #[test]
    fn a_label_launchd_does_not_hold_is_not_loaded() {
        let state = parse_label_print("mini", "x", "STADO_LABEL_DONE\tno\n");
        assert!(!state.loaded());
        assert!(state.pid.is_none());
    }

    #[test]
    fn the_bare_program_is_used_only_when_there_is_no_argv() {
        let stdout = "STADO_LABEL_DOMAIN\tsystem\n\
             STADO_LABEL_FIELD\tprogram\t/u/.stado/bin/stado\n\
             STADO_LABEL_DONE\tyes\n";
        let state = parse_label_print("mini", "x", stdout);
        assert_eq!(state.runs(), Some("/u/.stado/bin/stado"));
    }

    #[test]
    fn an_environment_line_is_never_echoed() {
        // The remote filter takes a fixed key list; anything else must not be
        // parsed into the report even if a host somehow emitted it.
        let stdout = "STADO_LABEL_DOMAIN\tsystem\n\
             STADO_LABEL_FIELD\tSKARBIEC_TOKEN\tsecret-value\n\
             STADO_LABEL_DONE\tyes\n";
        let state = parse_label_print("mini", "x", stdout);
        let rendered = state.to_json().to_string();
        assert!(!rendered.contains("secret-value"));
    }

    #[test]
    fn the_remote_program_asks_only_for_named_scalars() {
        assert!(LABEL_PRINT_SCRIPT.contains("key == \"pid\""));
        // The reader is launchctl and the only verb it is given is `print`.
        // The path is chosen per domain into `$launch`, so the program never
        // spells the binary and the verb next to each other.
        assert!(LABEL_PRINT_SCRIPT.contains("/bin/launchctl"));
        assert!(LABEL_PRINT_SCRIPT.contains("$launch print"));
        // Read-only: nothing in this program may act on the job.
        for verb in [
            "bootout",
            "bootstrap",
            "kickstart",
            "kill",
            "unload",
            "load",
        ] {
            assert!(!LABEL_PRINT_SCRIPT.contains(verb), "{verb} must not appear");
        }
    }

    #[test]
    fn an_unsupported_init_system_says_so() {
        let state = parse_label_print("box", "x", "STADO_LABEL_UNSUPPORTED\tFreeBSD\n");
        assert_eq!(state.unsupported.as_deref(), Some("FreeBSD"));
        assert!(!state.loaded());
    }
}
