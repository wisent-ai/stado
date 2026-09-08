//! What the box itself is doing: its uptime, its memory, where its disk went,
//! and which processes hold either.

use super::super::arguments::{
    FIGMA_EXPORT_LOG_STAT, FIGMA_EXPORT_WORK_TREE_SIZE, JOB_PROGRESS_TARGET_OPEN_FILES,
    JOB_PROGRESS_TARGET_SIZE, WEB_HOSTING_TARGET_OPEN_FILES, WEB_HOSTING_TARGET_SIZE,
};
use super::super::programs::STADO_CLI;
use super::super::ApprovedCommand;

pub const MACHINE_READS: &[ApprovedCommand] = &[
    ApprovedCommand {
        argv: &[STADO_CLI, "registry", "doctor"],
        why: "reads the installed build's registry verdict and the unit images on the \
              machine that owns them. A workstation cannot inspect another host's \
              executing images; the diagnostic must run there. This fixed command \
              changes no service, accepts no path or repair flag, and reports the \
              host's own findings without treating an unread image as agreement",
    },
    ApprovedCommand {
        argv: &["/usr/bin/uptime"],
        why: "reads kernel uptime and load counters; takes no argument and writes nothing",
    },
    ApprovedCommand {
        argv: &["/usr/sbin/sysctl", "vm.swapusage"],
        why: "reads the macOS swap file's total, used and free bytes; one fixed read-only \
              key, no path and no write. Added 2026-09-06: `vm_stat` further down this list \
              already answered the page counters, and page counters alone cannot say whether \
              a host is out of memory or merely paging. charless-mac-mini held 597 MiB free \
              with 4.7 GiB of its 6 GiB swap in use while every .NET runner on it failed to \
              start with E_OUTOFMEMORY",
    },
    ApprovedCommand {
        argv: &["/bin/df", "-h"],
        why: "reads mounted-filesystem statistics; -h is a fixed display unit, and with no \
              path argument it cannot be pointed at anything",
    },
    ApprovedCommand {
        argv: &["/usr/bin/du", "-xk", "-d", "2", "/"],
        why: "attributes a full root filesystem two directory levels deep; -x stays on one \
              filesystem, -k is a fixed unit, the depth and the root are fixed words, and du \
              writes nothing. Added 2026-08-19: the linux builder sat at 100% used and every \
              declared cleaner and reclaim stage measured zero, so the operator had no \
              sanctioned way to even name what was eating the disk",
    },
    ApprovedCommand {
        argv: &["/usr/bin/du", "-xk", "-d", "3", "/"],
        why: "attributes a full root filesystem one level below the existing depth-two report; \
              the depth and root remain fixed, read-only words. Added 2026-09-04 after the \
              Ubuntu builder reached 100% while depth two named 16 GiB in /root/.stado, \
              20 GiB in /home/ubuntu and 26 GiB in /mnt/wd16tb but could not identify any \
              directory a declared cleaner could safely own",
    },
    ApprovedCommand {
        argv: &["/usr/bin/du", "-xk", "-d", "2", "/root/.stado/work"],
        why: "attributes the root-owned Stado work tree two levels deep; the path and depth \
              are fixed and read-only. Added 2026-09-04 after the Ubuntu builder reached \
              100% with 13 GiB below this one managed root while every reclaim stage \
              reported zero items",
    },
    ApprovedCommand {
        argv: &["/usr/bin/du", "-xk", "-d", "2", "/home/ubuntu/.cache"],
        why: "attributes the Ubuntu service account's cache tree two levels deep; the path \
              and depth are fixed and read-only, so regenerable caches can be distinguished \
              from installed environments before a cleaner claims them",
    },
    ApprovedCommand {
        argv: &["/usr/bin/du", "-xk", "-d", "2", "/home/ubuntu/.local"],
        why: "attributes the Ubuntu service account's local data tree two levels deep; the \
              path and depth are fixed and read-only, separating installed programs from \
              build artifacts before any cleanup policy changes",
    },
    ApprovedCommand {
        argv: &["/usr/bin/du", "-xk", "-d", "2", "/mnt/wd16tb/stado"],
        why: "attributes the declared large-disk Stado tree two levels deep; the path and \
              depth are fixed and read-only, proving whether its 26 GiB belongs on the root \
              filesystem or to a mounted storage role before any relocation",
    },
    ApprovedCommand {
        argv: &["/usr/bin/du", "-xk", "-d", "1", "/private/tmp"],
        why: "attributes the OS scratch directory one level deep; -x stays on one filesystem, \
              -k is a fixed unit, the depth and the path are fixed words, and du writes \
              nothing. Added 2026-09-04: charless-mac-mini reached 1.1 GB free of 239 GB, \
              which took the object API, the registry authority and every Skarbiec \
              decryption on that host down at once, and the root-level attribution named \
              /private/tmp as the second largest consumer at 14.2 GB while every declared \
              cleaner and reclaim stage measured zero. Nothing in this table could say what \
              those bytes were, so they could neither be defended nor reclaimed",
    },
    ApprovedCommand {
        argv: &["/bin/ls", "-lt", "/private/tmp"],
        why: "lists the OS scratch directory's own entries with their modification times; the \
              path is a fixed word, no operator selector is appended, and ls writes nothing. \
              Sizes alone cannot separate a wedged product's live scratch from an abandoned \
              tree, and deleting an unclassified 14 GB is not a repair. Added 2026-09-04 \
              beside the du entry above, for the same outage",
    },
    ApprovedCommand {
        argv: &["/usr/bin/who"],
        why: "reads the login-session table; takes no argument and writes nothing",
    },
    ApprovedCommand {
        argv: &["/bin/launchctl", "list"],
        why: "lists the calling user's launchd jobs. `list` without a label is the read-only \
              verb; the mutating verbs (bootout, bootstrap, kickstart, enable) are absent from \
              this table and cannot be reached through it. This is the view the unmanaged \
              weles-api agent shows up in",
    },
    ApprovedCommand {
        argv: &[
            "/bin/ps", "ax", "-o", "pid", "-o", "ppid", "-o", "etime", "-o", "comm",
        ],
        why: "lists process identifiers, elapsed time, and executable names without command \
              arguments or environment values; it is read-only and cannot expose secret argv",
    },
    ApprovedCommand {
        argv: &["/usr/bin/top", "-l", "4", "-s", "10", "-o", "cpu"],
        why: "samples every process four times at ten-second intervals and orders the fixed \
              read-only report by CPU use. A single `ps` sample can legitimately catch an \
              I/O-bound worker at zero; this bounded thirty-second observation distinguishes \
              that moment from a process consuming no CPU throughout the interval. It takes \
              no pid, command text, file, or operator-supplied selector and writes nothing",
    },
    ApprovedCommand {
        argv: &[
            "/bin/ps", "ax", "-o", "pid", "-o", "rss", "-o", "pcpu", "-o", "comm",
        ],
        why: "reports resident memory per process, by executable name only. The two `ps` \
              entries around it show identity, parentage and elapsed time but never a byte \
              count, so the one question a thrashing host forces - which process ate the \
              memory - had no answer in this table at all. Added 2026-09-03: \
              charless-mac-mini was holding ~2.9 GB in the compressor with ~88 MB free and \
              3,277,146 swapouts, which stalled every fresh ssh session on it for 12-25 s \
              and tripped an unrelated preflight's hard timeout; `vm_stat` proved the \
              pressure was real but could not name a single owner of it. `-o rss` is a \
              kernel counter and `-o comm` is the executable's name; `-o command` - the \
              full argv, where tokens and passwords are passed - is deliberately NOT in \
              this table and cannot be reached through it. The selector is fixed to `ax` \
              and takes no pid, user, or file argument, so it cannot be pointed at \
              anything narrower or anywhere else",
    },
    ApprovedCommand {
        argv: FIGMA_EXPORT_LOG_STAT,
        why: "reads only byte size and modification epoch for the fixed manual Figma export \
              log inside the managed account's Stado work tree. Added 2026-09-04 because a \
              job can renew its lease for hours while redirecting every progress byte away \
              from the canonical zero-byte job log; without two measurements of this file \
              the fleet cannot distinguish useful work from a hang. The path and format are \
              compile-time constants, no operator word is appended, and stat writes nothing",
    },
    ApprovedCommand {
        argv: FIGMA_EXPORT_WORK_TREE_SIZE,
        why: "reads allocated KiB below only the fixed manual Figma export work tree. The \
              export writes its result below that tree while its redirected log can grow \
              independently, so measuring both at two times distinguishes output progress \
              from logging alone. The path, unit and recursion root are compile-time \
              constants, no operator word is appended, du stays within the managed account's \
              work tree, and the command writes nothing",
    },
    ApprovedCommand {
        argv: JOB_PROGRESS_TARGET_SIZE,
        why: "reads allocated KiB below only the inactive job-progress probe's standard-tagged \
              Cargo target. The full host inventory times out before it reports this managed \
              root; the fixed path and unit expose no source contents and du writes nothing",
    },
    ApprovedCommand {
        argv: JOB_PROGRESS_TARGET_OPEN_FILES,
        why: "lists open files only below the inactive job-progress probe's fixed Cargo target. \
              A forced tagged-cache prune has no process guard of its own, so this read proves \
              whether any process still holds that exact cache before it is considered. The \
              recursive root is compile-time data and lsof writes nothing",
    },
    ApprovedCommand {
        argv: WEB_HOSTING_TARGET_SIZE,
        why: "reads allocated KiB below only the web-hosting worktree's standard-tagged Cargo \
              target. The fixed managed root is the remaining large cache named by disk \
              inventory; no operator path is accepted and du writes nothing",
    },
    ApprovedCommand {
        argv: WEB_HOSTING_TARGET_OPEN_FILES,
        why: "lists open files only below the web-hosting worktree's fixed Cargo target. A \
              forced tagged-cache prune has no process guard of its own, so the cache remains \
              protected unless this exact read finds no holder. The recursive root is \
              compile-time data and lsof writes nothing",
    },
    ApprovedCommand {
        argv: &[
            "/bin/ps",
            "axww",
            "-o",
            "pid",
            "-o",
            "ppid",
            "-o",
            "cgroup:200",
            "-o",
            "comm",
        ],
        why: "lists process identifiers, executable names, and their Linux control-group paths \
              without command arguments or environment values; it is read-only and identifies \
              the exact systemd unit behind a duplicate queue agent",
    },
];
