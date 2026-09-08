//! What an installed release, a managed unit and a web runtime look like from
//! the host, and the one fixed run root a delivery prepares.

use super::super::arguments::{
    BRAMA_RUNNER_APPHOST_SIGNATURE, BRAMA_RUNNER_CORECLR_SIGNATURE, PROBIERZ_RUN_ROOT_CREATE,
    WELES_ADMISSION_CURRENT, WELES_ADMISSION_RELEASE_TREE, WELES_ADMISSION_VERSIONS,
    WELES_ADMISSION_WORKER_MODULES,
};
use super::super::programs::{CADDY_PROXY, NODE_RUNTIME, NPM_CLI};
use super::super::ApprovedCommand;

pub const SERVICE_AND_RUNTIME_READS: &[ApprovedCommand] = &[
    // The four reads a release that is installed but not running needs, added
    // 2026-09-02. On that evening `com.wisent.weles-admission` on
    // charless-mac-mini crash-looped on `Cannot find module
    // .../runtime/dist/worker/dispatch.js` while `stado release status
    // weles-worker` reported 0.5.57 committed and active. Three different
    // repairs hid behind that: the build had dropped the file, the install had
    // put it where the launcher does not look, or the launcher was resolving a
    // tree from an older release. Separating them is four facts about one
    // directory -- which digest `current` resolves to, which digests are
    // installed beside it, what the launcher sees inside the one it reaches,
    // and whether the compiled worker modules the API server imports are
    // there -- and this table could read none of them: `ls` existed only as
    // the fixed `ls /Applications`, and there is no `readlink`, `cat`, `find`
    // or `stat` entry. The whole diagnosis stopped on the symlink evidence
    // and said so.
    //
    // Each entry names the one service, because a path an operator supplies is
    // a path that can be `~/.ssh/id_ed25519`. `.stado/services` holds the
    // fleet's own installed release trees and nothing of the account's: no
    // documents, no keys, no credential files, and a directory name is not a
    // secret. All four are relative and are resolved by
    // [`home_rooted_script`] against the managed account's own home.
    ApprovedCommand {
        argv: WELES_ADMISSION_CURRENT,
        why: "prints the release directory `com.wisent.weles-admission` executes through. \
              The unit's program is that link plus a platform directory, so this name is the \
              whole answer to which release is running, and it is the fact `release status` \
              cannot give: that verb reports what a rollout recorded, and on 2026-09-02 the \
              two disagreed by four releases. `readlink` reads one link and writes nothing; \
              the path is a compile-time constant naming this one managed service",
    },
    ApprovedCommand {
        argv: WELES_ADMISSION_VERSIONS,
        why: "lists every release directory installed for that service beside the `current` \
              link, and, because `-l` renders a symlink with its target, the link and the \
              directory it names on the same page. This is what separates 'the rollout never \
              installed the release' from 'it installed it and left the link behind': the \
              installer keeps the previous version rather than deleting it, so a digest \
              present but unlinked is a rollback that happened and a digest absent is a \
              rollout that did not. `-l` is a display flag, the directory is fixed, and `ls` \
              writes nothing",
    },
    ApprovedCommand {
        argv: WELES_ADMISSION_RELEASE_TREE,
        why: "lists the inside of the release directory the launcher actually stands in: the \
              `payload` archive it unpacks from, the `runtime` tree it unpacks into, and the \
              modification times of both. That launcher unpacks only when the runtime carries \
              no ready marker, so a runtime older than its own payload is a tree pinned \
              incomplete, and a payload that is gone means the tree can never be re-derived \
              at all -- the difference between a release that will heal on the next start and \
              one that cannot. `-l` is a display flag, the directory is fixed, and `ls` writes \
              nothing",
    },
    ApprovedCommand {
        argv: WELES_ADMISSION_WORKER_MODULES,
        why: "lists the compiled worker modules in the runtime tree that service's launcher \
              actually resolves -- the directory the API server imports `dispatch.js` from, \
              reached through `current` exactly as the running process reaches it. A payload \
              proven to contain the file proves nothing about the tree under `current` if the \
              link points at a different release, which is the mistake this entry exists to \
              stop. It takes no flag and no operator path, lists names only, and writes \
              nothing",
    },
    // The three reads a web product's release and its unit need before either
    // one runs, added 2026-09-02. `stado web` builds a Node product on a fleet
    // builder with `npm ci` and runs it on a fleet host with `npm run start`,
    // and the public hostname in front of it is terminated by a
    // registry-managed Caddy unit. Each of those three facts is a property of
    // the machine that is true before the release is submitted, and none of
    // them could be asked of a host through this channel.
    //
    // All three probe absolute paths rather than the login shell's PATH, the
    // way the `uv --version` entry above does and for the same reason: a
    // non-interactive ssh login reads no profile, so a PATH lookup answers
    // `not found` on a host that carries the binary, which is the wrong answer
    // to a precondition check and the most expensive kind of wrong answer to
    // get.
    ApprovedCommand {
        argv: &[NODE_RUNTIME, "--version"],
        why: "prints the Node runtime's version, probing the absolute paths this fleet installs \
              it at rather than the login shell's PATH — which is a different question and \
              answers `not found` on a host that has the binary, because a non-interactive ssh \
              login reads no shell profile and the fleet's Node comes from Homebrew. A web \
              product's release builds with `npm ci` on whichever host the recipe's \
              `runner_platform` selects, and its unit runs `npm run start` on whichever host \
              the product is declared against, so a machine carrying no Node toolchain fails \
              the first inside a quality gate and the second at unit bootstrap. Until this \
              entry there was no sanctioned way to ask either host whether it has a Node \
              toolchain at all: the question got answered by reading a release log after a \
              build had already failed, which spends a whole submit to learn one fact that was \
              true of the machine before the release started. `--version` takes no argument, \
              installs nothing, resolves no registry, and runs no package script",
    },
    ApprovedCommand {
        argv: &[NPM_CLI, "--version"],
        why: "prints the npm CLI's version, probed the same way, for the other half of the same \
              precondition. The runtime and the package manager are separate binaries, a \
              partial or hand-rolled install leaves a host with one and not the other, and it \
              is npm — not node — that a web release invokes: `npm ci` in the quality gate and \
              `npm run start` in the unit. Its version is also the fact that decides whether \
              `npm ci` can read the product's checked-in `package-lock.json` at all, since a \
              lockfile written by a newer npm than the host carries is refused rather than \
              honoured. That is the difference between 'this builder cannot build a Node \
              product' and 'this product's build is broken', and before this entry the fleet \
              learned which one it was facing from a failed release's log. `--version` prints \
              and exits: it contacts no registry, writes no cache, and runs no lifecycle \
              script",
    },
    ApprovedCommand {
        argv: &[CADDY_PROXY, "version"],
        why: "prints the version of the Caddy binary a host carries, probed at the same \
              absolute paths rather than through the login shell's PATH. The public web edge \
              terminates TLS for a product hostname with a registry-managed Caddy unit, so \
              whether a host already carries that binary is the precondition of installing it: \
              a host that has it needs a unit and a configuration written for it, and a host \
              that does not needs the binary itself first, which is a different repair by a \
              different mechanism. Asking after the fact means learning the answer from a unit \
              that will not start, with the hostname already published and no certificate \
              behind it. `version` is Caddy's own read-only subcommand: it loads no \
              configuration, binds no port and starts no server, unlike `run`, `start` and \
              `reload`, none of which is in this table",
    },
    ApprovedCommand {
        argv: &[
            "/usr/bin/systemctl",
            "list-units",
            "--type",
            "service",
            "--all",
            "--no-pager",
            "--no-legend",
        ],
        why: "lists this host's systemd services, the Linux counterpart of the `launchctl \
              list` entry above. Added 2026-09-03: the fleet's one linux-amd64 builder had \
              been running a two-day-old stado image that refuses today's registry document \
              (`policy:ValueError`), so its own janitor never learned a low watermark and it \
              claimed nothing -- every release build for that platform queued behind it. \
              Naming the unit that holds that process is the first step of the repair, and \
              this table could not name a systemd unit at all: `launchctl list` answers only \
              on macOS. `list-units` is systemd's read-only verb with every selector fixed \
              here; the mutating verbs (start, stop, restart, enable, daemon-reload) are \
              absent from this table and cannot be reached through it",
    },
    ApprovedCommand {
        argv: &["/bin/cat", "/etc/ssh/sshd_config"],
        why: "reads the Ubuntu OpenSSH server's fixed primary configuration file so a \
              server-side command or session override can be attributed; the path is fixed, \
              no included file or operator-supplied path is followed, and cat writes nothing",
    },
    ApprovedCommand {
        argv: &[
            "/usr/bin/journalctl",
            "--unit",
            "ssh.service",
            "--lines",
            "200",
            "--no-pager",
        ],
        why: "reads the last 200 records owned by Ubuntu's active OpenSSH systemd unit, with \
              a fixed unit and bound output; journalctl's read-only form neither changes the \
              service nor follows future records",
    },
    ApprovedCommand {
        argv: &[
            "/usr/bin/systemctl",
            "list-unit-files",
            "--type",
            "service",
            "--no-pager",
            "--no-legend",
        ],
        why: "lists the systemd service unit FILES installed on this host, which is a \
              different question from `list-units` above: a unit whose file exists but was \
              never loaded appears only here, and that is exactly the shape an undeclared \
              queue agent takes. Read-only, every selector fixed, and it takes no unit name",
    },
    ApprovedCommand {
        argv: &[
            "/usr/bin/systemctl",
            "show",
            "stado-host-beacon.service",
            "-p",
            "NeedDaemonReload",
            "-p",
            "Type",
            "-p",
            "TriggeredBy",
            "-p",
            "Result",
            "-p",
            "ExecMainStartTimestamp",
            "-p",
            "ExecMainStatus",
            "-p",
            "FragmentPath",
            "-p",
            "DropInPaths",
            "-p",
            "EnvironmentFiles",
        ],
        why: "reads the loaded beacon definition, its source and override paths, and its \
              last publication result. This distinguishes a stale manager definition from \
              a later environment override without restarting anything. The unit and \
              properties are fixed and credential values are not read",
    },
    ApprovedCommand {
        argv: &[
            "/usr/bin/systemctl",
            "cat",
            "stado-host-beacon.service",
            "--no-pager",
        ],
        why: "prints the fixed beacon unit fragment followed by every systemd drop-in in \
              precedence order. The loaded property view above names override paths but does \
              not show which one still supplies an obsolete API URL after the managed base \
              environment file was corrected. `cat` is read-only, the unit name and \
              no-pager flag are fixed, and no operator path or arbitrary argument reaches \
              systemd",
    },
    ApprovedCommand {
        argv: BRAMA_RUNNER_APPHOST_SIGNATURE,
        why: "reads the registered Brama runner's existing apphost signature and entitlements. \
              CoreCLR error 0x8007000C can occur with a valid signature, so verification alone \
              does not explain the loader failure. The path is fixed under the managed account; \
              display mode changes no signature and requests no certificate or consent",
    },
    ApprovedCommand {
        argv: BRAMA_RUNNER_CORECLR_SIGNATURE,
        why: "reads the same runner's CoreCLR library signing identity so a loader failure can \
              be compared with its apphost rather than guessed from the HRESULT. The fixed \
              display-only invocation neither loads the library nor modifies it",
    },
    ApprovedCommand {
        argv: &["/usr/bin/csrutil", "status"],
        why: "reads macOS System Integrity Protection status while diagnosing a CoreCLR \
              memory-access refusal. This status-only invocation changes no boot setting, \
              opens no consent window, and never reboots the host",
    },
    ApprovedCommand {
        argv: PROBIERZ_RUN_ROOT_CREATE,
        why: "creates only the fixed `$HOME/.stado/work/runs` parent used by target-scoped \
              Probierz deliveries. Added 2026-09-06 because the byk-auth journey previously \
              opened a raw SSH shell only to create its run root before rsync, bypassing the \
              target channel Stado owns. The operator supplies no path or run id: the fixed \
              script derives HOME on the target, sets umask 077, refuses symlinked or \
              foreign-owned components, creates missing components one at a time, and fixes \
              the final root at mode 0700. Canonical per-run UUID children are admitted by \
              `stado host deliver`, not by this allowlist entry",
    },
];
