//! The host passes and host commands Stado runs on a Mac ask macOS for no
//! privacy-gated data the host does not declare.
//!
//! macOS keeps a privacy decision per code identity, so a program it has
//! never seen gets every question afresh: the real `stado` this build made
//! is copied into the checkout's build directory and signed there with an
//! identifier no earlier run used. That copy runs, in this account's real
//! home (the only place macOS gates), the periodic passes of the always-on
//! host process — one janitor pass as a preview (`space reclaim --local
//! --dry-run`, its build-cache walk included) and one health beacon (`host
//! beacon collect`, not published) — and the read-only host commands that
//! walk the home (`space report`, `host build-caches`, `host spotlight`
//! without `--apply`) against this host's own registry entry. Nothing is
//! written to the registry, the store or the home.
//!
//! The assertion reads the unified log: every request `tccd` recorded since
//! the copy was signed whose accessing or responsible program is the copy.
//! macOS holds the app a command was started from responsible for it and
//! for every child it starts, so each run of the copy is spawned disclaiming
//! that responsibility, as launchd starts the host process: the copy is then
//! the responsible program for its own opens and for the `find` and `du`
//! its scripts run. No request may reach protected data — none through
//! `sandboxd` other than the Full Disk Access preflight that precedes every
//! protected open, and none that showed a dialog. The copy is no program any
//! `targets[].privacy_grants` names, so even the consent-gated folders are
//! declared for it nowhere.
#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::ffi::CString;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};

use nix::libc;

extern "C" {
    /// libsystem's spawn attribute that makes the new process responsible
    /// for itself, as launchd makes a job's program.
    fn responsibility_spawnattrs_setdisclaim(
        attrs: *mut libc::posix_spawnattr_t,
        disclaim: libc::c_int,
    ) -> libc::c_int;
    /// libsystem's file action that starts the new process in a directory.
    fn posix_spawn_file_actions_addchdir_np(
        actions: *mut libc::posix_spawn_file_actions_t,
        path: *const libc::c_char,
    ) -> libc::c_int;
}

/// The Full Disk Access check `sandboxd` makes before every narrower one; on
/// its own it shows no dialog.
const ALL_FILES_SERVICE: &str = "kTCCServiceSystemPolicyAllFiles";

/// The copy of the real binary under a code identity of its own, removed
/// with its directory on drop.
struct Probe {
    root: PathBuf,
    program: PathBuf,
}

impl Probe {
    fn new() -> Self {
        let unique = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the clock reads after the epoch")
                .as_nanos()
        );
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("privacy-tcc-{unique}"));
        std::fs::create_dir_all(&root).expect("create the probe's directory");
        let program = root.join("stado");
        std::fs::copy(env!("CARGO_BIN_EXE_stado"), &program).expect("copy the real stado");
        let signed = run(Command::new("/usr/bin/codesign").args([
            "--sign",
            "-",
            "--force",
            "--identifier",
            &format!("stado-tcc-probe-{unique}"),
            program.to_str().expect("the probe path is UTF-8"),
        ]));
        assert!(
            signed.status.success(),
            "codesign could not give the copy its own identity: {}",
            String::from_utf8_lossy(&signed.stderr)
        );
        Self { root, program }
    }

    /// The copy, run in this account's own environment and home, responsible
    /// for itself and its children; its output is kept in the probe's
    /// directory under `name`.
    fn stado(&self, name: &str, arguments: &[&str]) -> Output {
        let stdout = self.root.join(format!("{name}.out"));
        let stderr = self.root.join(format!("{name}.err"));
        let text = |value: &str| CString::new(value).expect("an argument holds no NUL");
        let path = |path: &Path| text(path.to_str().expect("the probe path is UTF-8"));
        let program = path(&self.program);
        let mut argv: Vec<CString> = vec![program.clone()];
        argv.extend(arguments.iter().map(|argument| text(argument)));
        let mut argv_pointers: Vec<*mut libc::c_char> = argv
            .iter()
            .map(|argument| argument.as_ptr().cast_mut())
            .collect();
        argv_pointers.push(std::ptr::null_mut());
        let environment: Vec<CString> = std::env::vars_os()
            .map(|(key, value)| {
                let mut pair = key.into_encoded_bytes();
                pair.push(b'=');
                pair.extend(value.into_encoded_bytes());
                CString::new(pair).expect("an environment entry holds no NUL")
            })
            .collect();
        let mut environment_pointers: Vec<*mut libc::c_char> = environment
            .iter()
            .map(|entry| entry.as_ptr().cast_mut())
            .collect();
        environment_pointers.push(std::ptr::null_mut());
        let (null, out, err) = (text("/dev/null"), path(&stdout), path(&stderr));
        let root = path(&self.root);
        let mut pid: libc::pid_t = 0;
        // SAFETY: every pointer handed to posix_spawn outlives the call, and
        // the attribute and file-action objects are initialised before use
        // and destroyed after it.
        let spawned = unsafe {
            let mut attrs: libc::posix_spawnattr_t = std::ptr::null_mut();
            let mut actions: libc::posix_spawn_file_actions_t = std::ptr::null_mut();
            assert_eq!(libc::posix_spawnattr_init(&mut attrs), 0);
            assert_eq!(responsibility_spawnattrs_setdisclaim(&mut attrs, 1), 0);
            assert_eq!(libc::posix_spawn_file_actions_init(&mut actions), 0);
            let write = libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC;
            libc::posix_spawn_file_actions_addopen(
                &mut actions,
                0,
                null.as_ptr(),
                libc::O_RDONLY,
                0,
            );
            libc::posix_spawn_file_actions_addopen(&mut actions, 1, out.as_ptr(), write, 0o600);
            libc::posix_spawn_file_actions_addopen(&mut actions, 2, err.as_ptr(), write, 0o600);
            posix_spawn_file_actions_addchdir_np(&mut actions, root.as_ptr());
            let spawned = libc::posix_spawn(
                &mut pid,
                program.as_ptr(),
                &actions,
                &attrs,
                argv_pointers.as_ptr(),
                environment_pointers.as_ptr(),
            );
            libc::posix_spawn_file_actions_destroy(&mut actions);
            libc::posix_spawnattr_destroy(&mut attrs);
            spawned
        };
        assert_eq!(spawned, 0, "the copy could not be spawned: errno {spawned}");
        let mut status: libc::c_int = 0;
        // SAFETY: `pid` is this process's own child, reaped exactly once.
        let reaped = unsafe { libc::waitpid(pid, &mut status, 0) };
        assert_eq!(reaped, pid, "the copy's exit could not be read");
        Output {
            status: ExitStatus::from_raw(status),
            stdout: std::fs::read(&stdout).expect("read the copy's stdout"),
            stderr: std::fs::read(&stderr).expect("read the copy's stderr"),
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.root) {
            eprintln!("{} was not removed: {error}", self.root.display());
        }
    }
}

/// One program run to its end through the product's one wait mechanism.
fn run(command: &mut Command) -> Output {
    command.stdin(Stdio::null());
    stado_wait::output(command).expect("the program starts")
}

/// The local wall-clock second, in the form `log show --start` reads.
fn now() -> String {
    let output = run(Command::new("/bin/date").arg("+%Y-%m-%d %H:%M:%S"));
    String::from_utf8(output.stdout)
        .expect("date prints text")
        .trim()
        .to_string()
}

/// A command's stdout as JSON, the command having succeeded.
fn json_of(output: &Output, what: &str) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{what} exited {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{what} printed no JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

/// The value of `name=` inside one log line, up to the next comma or brace.
fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let start = line.find(&format!("{name}="))? + name.len() + 1;
    let rest = &line[start..];
    Some(&rest[..rest.find([',', '}']).unwrap_or(rest.len())])
}

/// One request `tccd` logged, joined from its lines by daemon and message id.
#[derive(Default, Debug)]
struct Request {
    service: Option<String>,
    attribution: Option<String>,
    prompted: bool,
}

/// Every request `tccd` logged since `start`, by daemon and message id.
fn requests_since(start: &str) -> BTreeMap<String, Request> {
    let output = run(Command::new("/usr/bin/log").args([
        "show",
        "--start",
        start,
        "--style",
        "compact",
        "--predicate",
        "subsystem == \"com.apple.TCC\" AND category == \"access\" AND eventMessage BEGINSWITH \"AUTHREQ_\"",
    ]));
    assert!(
        output.status.success(),
        "log show exited {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let mut requests: BTreeMap<String, Request> = BTreeMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let (Some(daemon), Some(id)) = (
            line.split("tccd[")
                .nth(1)
                .and_then(|rest| rest.split(':').next()),
            field(line, "msgID"),
        ) else {
            continue;
        };
        let request = requests.entry(format!("{daemon}:{id}")).or_default();
        if line.contains("AUTHREQ_CTX:") {
            request.service = field(line, "service").map(str::to_string);
        } else if line.contains("AUTHREQ_ATTRIBUTION:") {
            request.attribution = Some(line.to_string());
        } else if line.contains("AUTHREQ_PROMPTING:") {
            request.prompted = true;
        }
    }
    requests
}

#[test]
fn host_passes_and_host_commands_ask_macos_for_nothing_undeclared() {
    let probe = Probe::new();
    let start = now();

    // The janitor's pass, as a preview: the same lock and walks, nothing
    // removed. The build-cache walk must have run, or the home was never read.
    let reclaim = json_of(
        &probe.stado(
            "reclaim",
            &["space", "reclaim", "--local", "--dry-run", "--json"],
        ),
        "space reclaim --local --dry-run",
    );
    let walk = &reclaim["cleaners"]["build_caches"];
    assert!(
        walk["skipped"].get("walk_in_progress").is_none(),
        "the build-cache walk did not run: another janitor holds its walk lock under \
         ~/.cache/wisent-compute, so the home was not read: {walk}"
    );
    assert!(
        walk["scanned_items"]
            .as_i64()
            .is_some_and(|items| items > 0),
        "the build-cache walk read nothing of the home: {walk}"
    );

    // The health beacon the host process publishes every interval; its
    // `host` is this machine's registry identity for the commands below.
    let beacon = json_of(
        &probe.stado("beacon", &["host", "beacon", "collect"]),
        "host beacon collect",
    );
    let host = beacon["host"]
        .as_str()
        .expect("the beacon names its host")
        .to_string();
    for (folder, entry) in beacon["privacy"]["folders"]
        .as_object()
        .expect("the beacon carries the privacy folders")
    {
        assert_eq!(
            entry["state"], "not measured",
            "the beacon opened {folder} although no grant names the probe: {entry}"
        );
    }

    // The host commands that walk this host's home.
    let report = probe.stado("space-report", &["space", "report", &host, "--json"]);
    json_of(&report, "space report");
    let caches = probe.stado("build-caches", &["host", "build-caches", &host]);
    assert!(
        caches.status.success(),
        "host build-caches exited {}: {}",
        caches.status,
        String::from_utf8_lossy(&caches.stderr)
    );
    let spotlight = probe.stado("spotlight", &["host", "spotlight", &host, "--json"]);
    json_of(&spotlight, "host spotlight");

    let program = probe.program.to_str().expect("the probe path is UTF-8");
    let reached: Vec<String> = requests_since(&start)
        .into_values()
        .filter(|request| {
            request
                .attribution
                .as_deref()
                .is_some_and(|line| line.contains(&format!("binary_path={program}")))
        })
        .filter(|request| {
            let sandboxed = request
                .attribution
                .as_deref()
                .is_some_and(|line| line.contains("identifier=com.apple.sandboxd"));
            request.prompted || (sandboxed && request.service.as_deref() != Some(ALL_FILES_SERVICE))
        })
        .map(|request| format!("{request:?}"))
        .collect();
    assert!(
        reached.is_empty(),
        "macOS recorded {} request(s) for protected data against the probe: {}",
        reached.len(),
        reached.join("; ")
    );
}
