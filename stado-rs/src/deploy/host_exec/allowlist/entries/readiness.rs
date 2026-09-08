//! What this host is, what it is listening on, and which programs a
//! placement needs installed before it can claim a slot here.

use super::super::programs::{
    ANDROID_DEBUG_BRIDGE, APPIUM_CLI, CARGO_CLI, CUA_DRIVER, GIT_CLI, TMUX_CLI, UV_INSTALLER,
};
use super::super::ApprovedCommand;

pub const READINESS_READS: &[ApprovedCommand] = &[
    ApprovedCommand {
        argv: &["/usr/sbin/netstat", "-anv", "-p", "tcp"],
        why: "reads the kernel TCP socket table without connecting to any endpoint; fixed \
              flags expose listeners and owning processes but accept no remote address",
    },
    ApprovedCommand {
        argv: &["/usr/sbin/lsof", "-nP", "-iTCP", "-sTCP:LISTEN"],
        why: "names the process behind every listening TCP port. `netstat -anv -p tcp` above \
              never shows an owner, so the one question the fleet asks most often - which \
              process holds this port - was answered over ssh instead. The flags fix the \
              selection to listeners and take no argument, so it cannot be pointed at a file, \
              a user, or a remote address",
    },
    ApprovedCommand {
        argv: &["/usr/bin/crontab", "-l"],
        why: "prints the calling account's own crontab. `-l` is the read-only verb and takes \
              no argument: `-e` opens an editor, `-r` deletes the table, and neither is in \
              this table nor reachable through it; `-u <user>` would read another account's \
              and is deliberately absent. Added 2026-08-31: a process nobody could name had \
              been overwriting charless-mac-mini's janitor state file every four minutes \
              since at least that morning, with the default outcome and no writer \
              attribution, while the queue agent's own broadcast reported a healthy pass in \
              the same second. It is not a launchd job - 47 undeclared fleet labels on that \
              host, none of them a janitor - and it holds the run lock too briefly to be \
              caught by sampling, which leaves a periodic table as the only remaining place \
              it can be declared. Every reader in this group could see the file change and \
              none could name the writer",
    },
    ApprovedCommand {
        argv: &["/usr/bin/uname", "-a"],
        why: "prints kernel identification; -a only widens the fields, and there is no input \
              to interpret",
    },
    ApprovedCommand {
        argv: &["/usr/bin/sw_vers"],
        why: "prints the macOS product and build version; takes no argument and writes nothing",
    },
    ApprovedCommand {
        argv: &["/usr/bin/vm_stat"],
        why: "prints Mach virtual-memory statistics; takes no argument and writes nothing",
    },
    ApprovedCommand {
        argv: &["/bin/hostname", "-f"],
        why: "prints the fully qualified hostname; -f only selects the long form. Reading it \
              is how a registry `hostnames` entry gets checked against the box itself",
    },
    ApprovedCommand {
        argv: &["/usr/bin/id"],
        why: "prints the login user's uid, gid and groups; takes no argument and writes nothing",
    },
    ApprovedCommand {
        argv: &["/bin/date", "-u"],
        why: "prints the current UTC clock; -u only selects the timezone. Clock skew is a real \
              cause of refused ssh keys and failed storage authentication, so it is worth \
              being able to read",
    },
    ApprovedCommand {
        argv: &["/usr/bin/defaults", "read", "MobileMeAccounts"],
        why: "prints which Apple accounts the login user is signed into. Some work runs only \
              on the machine that holds an identity -- a two-factor prompt appears on the \
              trusted device and nowhere else -- so `stado identity verify` must check the \
              binding rather than trust a declaration nothing re-reads. The domain is fixed, \
              `read` is the read-only verb, and the output carries account identifiers, never \
              tokens or passwords",
    },
    ApprovedCommand {
        argv: &["/usr/bin/dscl", ".", "-list", "/Users"],
        why: "lists the local account names on the host. An identity binding may name a user \
              other than the login user, and `defaults read` answers only for whoever the \
              channel logs in as, so such a binding reports unknown forever. Whether that user \
              exists at all is the question that separates a real gap from a declaration \
              nobody can ever satisfy. `.` is the local node, `-list` is the read-only verb, \
              and account names are not secrets",
    },
    ApprovedCommand {
        argv: &["/usr/bin/xcrun", "simctl", "list", "devices", "available"],
        why: "lists installed iOS Simulator runtimes and devices without booting or mutating \
              one; this is the prerequisite check for native iOS capture placement",
    },
    ApprovedCommand {
        argv: &["/usr/bin/xcrun", "devicectl", "list", "devices"],
        why: "lists Apple devices visible to CoreDevice without installing, launching, or \
              changing anything; physical-device availability decides whether App Store \
              binaries can be captured rather than simulator-only builds",
    },
    ApprovedCommand {
        argv: &["/bin/ls", "/Applications"],
        why: "lists system-wide installed application bundle names under the fixed public \
              Applications directory; it reads no user documents and changes nothing",
    },
    ApprovedCommand {
        argv: &["/usr/bin/which", "adb"],
        why: "reports whether Android platform-tools are on the managed login's PATH; it takes \
              a fixed executable name, reads no application state, and writes nothing",
    },
    // The four crawl prerequisites, added 2026-09-03. Spis's crawl coordinator
    // preflights a placement host through this channel before it submits any
    // job, and for these four the channel answered "not an approved host-exec
    // command". That refusal is indistinguishable from "the program is
    // missing", so the 2026-09-01 crawl run recorded fifteen catalogs as
    // preflight_failed without anybody being able to say which of the two it
    // was. Each entry prints a version or a self-check, takes no
    // operator-supplied word, installs nothing and mutates nothing.
    ApprovedCommand {
        argv: &[APPIUM_CLI, "--version"],
        why: "prints the installed Appium server's version, probing the absolute paths this \
              fleet installs it at rather than the non-interactive ssh PATH -- a different \
              question that answers `not found` on a host that has the binary. It is the \
              precondition of every iOS and Android capture placement: without Appium there \
              is no driver to open an installed application with. `--version` starts no \
              server, opens no device and writes nothing",
    },
    ApprovedCommand {
        argv: &[APPIUM_CLI, "driver", "list", "--installed"],
        why: "lists which Appium drivers are actually installed, which is the half of mobile \
              readiness a version cannot answer: XCUITest for iOS and UiAutomator2 for \
              Android are separate installs, and a placement fails at the first command \
              without them. `list --installed` reads the local driver manifest; the forms \
              that change anything (`driver install`, `uninstall`, `update`) are absent from \
              this table and unreachable through it, because the allowlist matches an entry \
              exactly and never appends operator words",
    },
    ApprovedCommand {
        argv: &[ANDROID_DEBUG_BRIDGE, "version"],
        why: "prints the installed Android Debug Bridge's version from the absolute paths the \
              fleet installs platform-tools at. `which adb` above answers the PATH question \
              and returns nothing on a Homebrew install reached over ssh, which is why both \
              exist. `version` contacts no device and starts no server beyond adb's own \
              local one",
    },
    ApprovedCommand {
        argv: &[ANDROID_DEBUG_BRIDGE, "devices", "-l"],
        why: "lists the Android devices and emulators this host can currently see, with their \
              transport and model. It is the placement question for the Android family: a \
              host with adb and no device cannot capture anything. The listing names devices, \
              not their contents, and changes nothing on them",
    },
    ApprovedCommand {
        argv: &[CUA_DRIVER, "doctor", "--json"],
        why: "runs the Cua Driver's own read-only self-check and prints it as JSON: whether \
              the driver is installed and whether this host's accessibility and \
              screen-recording grants are in place. That is the exact precondition of the \
              macOS and desktop capture families, and `~/.stado/bin/install-cua-driver` is \
              what would repair it. `doctor` opens no application and grants nothing itself",
    },
    ApprovedCommand {
        argv: &[CARGO_CLI, "--version"],
        why: "prints the installed Rust toolchain's cargo version from the paths this fleet \
              installs Rust at, shared-toolchain prefix first. Every Spis crawl worker runs \
              as `cargo run --release` at a pinned revision on the placement host, so this \
              one fact decides whether the terminal, command-line, documentation and native \
              families can execute there at all. `--version` compiles nothing, fetches \
              nothing and writes nothing",
    },
    ApprovedCommand {
        argv: &[GIT_CLI, "--version"],
        why: "prints git's version from the paths a real installation puts it at, and \
              deliberately NOT from `/usr/bin/git`, which on macOS is the `xcode-select` \
              shim: on a host without the Command Line Tools that path opens the installer \
              WINDOW instead of answering, so the obvious probe is the one spelling that \
              could raise a consent dialog on an unattended fleet host. Spis's terminal \
              family builds its fixture repository with git, so this decides whether that \
              family can run at all, and the resolved path is what the worker's command is \
              then built from rather than a bare name a non-login shell cannot find. \
              `--version` reads no repository, touches no working tree and writes nothing",
    },
    ApprovedCommand {
        argv: &[TMUX_CLI, "-V"],
        why: "prints the tmux version from the absolute paths this fleet installs it at. Spis's \
              command-line and terminal families drive the product under test inside a tmux \
              session, so this is their precondition exactly as cargo is every worker's -- and \
              until this entry existed the question could not be asked at all: `tmux -V` came \
              back `not an approved host-exec command`, so both families refused every host and \
              the refusal read as a missing program. `-V` starts no server, attaches to no \
              session and writes nothing",
    },
    ApprovedCommand {
        argv: &[UV_INSTALLER, "--version"],
        why: "prints the uv package installer's version, probing the two absolute paths Weles's \
              kimi login trajectory itself probes rather than the login shell's PATH -- which \
              is a different question and answers `not found` on a host that has the binary. \
              Added 2026-09-02: that trajectory now resolves a pinned Kimi CLI version and \
              installs it through uv when the host carries a different one, and \
              charless-mac-mini carries a different one, so whether that repair can complete \
              there is entirely this one fact. `--version` takes no argument and installs \
              nothing",
    },
    ApprovedCommand {
        argv: &[
            "/usr/sbin/sysctl",
            "-n",
            "kern.maxproc",
            "kern.maxprocperuid",
        ],
        why: "reads two named kernel tunables — the system-wide and per-uid process ceilings — \
              and nothing else. `-n` prints values without names, the two keys are compile-time \
              constants rather than an operator-supplied name, and neither carries secret data. \
              This is the pair that says whether a host refusing to fork is out of process slots \
              or actually wedged; without it `host inventory` reporting probe_failed has no \
              follow-up question",
    },
    ApprovedCommand {
        argv: &["/bin/ps", "ax", "-o", "user", "-o", "pid", "-o", "comm"],
        why: "lists which login user owns each process, by executable name only. `-o comm` is \
              the executable's name; `-o command` — the full argv, where tokens and passwords \
              are passed — is deliberately NOT in this table and cannot be reached through it, \
              because the allowlist matches an entry exactly and never appends operator words. \
              Answers 'whose process is holding that port', which the pid-only listing cannot",
    },
];
