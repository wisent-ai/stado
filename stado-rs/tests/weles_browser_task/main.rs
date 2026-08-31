//! `stado host weles-browser-task` against a real env file.
//!
//! Every test drives the built `stado` binary. The registry target's
//! `hostnames` name THIS machine, so the allowlist is read through the same
//! `service file-fetch` path the ssh branch uses and the same `/bin/bash -s`
//! runs the remote script — byte-identical either way, only the hop
//! disappears. HOME is a tempdir, so the env file being read is real state
//! this test made.
//!
//! There is no fake Weles API here. These tests defend the half that must
//! happen BEFORE any host is enqueued: the allowlist gate. That is the whole
//! point of the command — `host weles-capture` sends `generic_capture` to a
//! worker whose allowlist does not carry it, and the job is accepted and
//! dropped. A refusal has to arrive here, naming the action and the host.
//!
//! What is defended: an action the host's allowlist does not carry is refused
//! with the action, the host and the general action that does exist in the
//! sentence; a 4488-character allowlist is read whole rather than clamped to
//! its first entries the way `env-show` would; a missing allowlist is refused
//! rather than treated as permissive; an allowed action passes the gate and
//! fails later for a reason that is plainly about reaching Weles; and login is
//! off unless asked for.

use std::io::Write;
use std::process::{Command, Output};

struct Fleet {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Fleet {
    fn new() -> Self {
        let fleet = Self {
            home: tempfile::tempdir().unwrap(),
            storage: tempfile::tempdir().unwrap(),
        };
        let hostname = String::from_utf8(Command::new("hostname").output().unwrap().stdout)
            .unwrap()
            .trim()
            .to_ascii_lowercase();
        let registry = serde_json::json!({
            "schema_version": 2,
            "targets": [{
                "name": "here",
                "kind": "local",
                "ssh": "nobody@127.0.0.1",
                "release_platform": platform(),
                "hostnames": [hostname],
                "slots": 1,
                "services": []
            }],
            "coordinators": []
        });
        std::fs::write(
            fleet.storage.path().join("registry.json"),
            serde_json::to_string_pretty(&registry).unwrap(),
        )
        .unwrap();
        fleet
    }

    /// Write the worker env file whose allowlist decides what the host takes.
    fn env_file(&self, body: &str) {
        let directory = self.home.path().join(".config/weles");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("worker.env");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(body.as_bytes()).unwrap();
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.storage.path())
            .env(
                "STADO_CONFIG",
                self.storage.path().join("no-such-config.json"),
            )
            .env("WC_PROVIDERS", "local")
            .env("WC_VAST_AUTO_LIST", "false")
            .output()
            .expect("stado binary runs")
    }

    fn task(&self, extra: &[&str]) -> Output {
        let mut args = vec![
            "host",
            "weles-browser-task",
            "here",
            "--url",
            "https://accounts.google.com/",
            "--objective",
            "sign in and report the outcome",
            "--session-label",
            "oko-calendar",
        ];
        args.extend_from_slice(extra);
        self.stado(&args)
    }
}

fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("no release platform mapping for {os}-{arch}"),
    }
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// An allowlist of the real size, so the reader is exercised against the
/// length that broke `env-show`: that command clamps a reported value at 400
/// characters, and charless-mac-mini's list is 4488.
fn long_allowlist(include: &[&str]) -> String {
    let mut actions: Vec<String> = include.iter().map(|name| (*name).to_string()).collect();
    for index in 0..250 {
        actions.push(format!("filler_action_{index:03}"));
    }
    let list = actions.join(",");
    assert!(
        list.len() > 400,
        "the fixture must exceed env-show's 400-character clamp"
    );
    format!("WELES_ACTION_ALLOWLIST={list}\nWELES_HEADLESS=1\n")
}

#[test]
fn an_action_the_host_does_not_carry_is_refused_naming_it_and_the_host() {
    let fleet = Fleet::new();
    fleet.env_file(&long_allowlist(&[
        "generic_browser_task",
        "generic_saved_task",
        "apple_login",
    ]));
    // The exact case: this is what `host weles-capture` hard-codes.
    let out = fleet.task(&["--action", "generic_capture"]);
    assert!(!out.status.success(), "{}", said(&out));
    let text = said(&out);
    assert!(text.contains("generic_capture"), "{text}");
    assert!(text.contains("here"), "{text}");
    assert!(
        text.contains("generic_browser_task"),
        "the refusal must name the general action that does exist:\n{text}"
    );
    assert!(
        text.contains("would refuse"),
        "the refusal must say the worker would refuse it:\n{text}"
    );
}

#[test]
fn the_whole_allowlist_is_read_rather_than_clamped_the_way_env_show_clamps() {
    let fleet = Fleet::new();
    // `generic_browser_task` is placed LAST, past the 400-character clamp, so
    // a reader that truncated would refuse a legitimate action.
    let mut actions: Vec<String> = (0..250).map(|i| format!("filler_action_{i:03}")).collect();
    actions.push("generic_browser_task".to_string());
    let list = actions.join(",");
    assert!(list.len() > 400);
    fleet.env_file(&format!("WELES_ACTION_ALLOWLIST={list}\n"));

    let out = fleet.task(&[]);
    let text = said(&out);
    // It must get PAST the gate: no allowlist complaint about this action.
    assert!(
        !text.contains("does not accept the action"),
        "an action past the clamp was refused, so the allowlist was truncated:\n{text}"
    );
}

#[test]
fn a_missing_allowlist_is_refused_rather_than_treated_as_permissive() {
    let fleet = Fleet::new();
    fleet.env_file("WELES_HEADLESS=1\n");
    let out = fleet.task(&[]);
    assert!(!out.status.success());
    let text = said(&out);
    assert!(
        text.contains("declares no WELES_ACTION_ALLOWLIST"),
        "{text}"
    );
}

#[test]
fn an_unreadable_env_file_is_refused_before_anything_is_enqueued() {
    let fleet = Fleet::new();
    // No env file at all.
    let out = fleet.task(&[]);
    assert!(!out.status.success());
    let text = said(&out);
    assert!(
        text.contains("which actions this worker accepts"),
        "the refusal must say why the allowlist could not be learned:\n{text}"
    );
    assert!(text.contains("missing"), "{text}");
}

#[test]
fn an_allowed_action_passes_the_gate_and_fails_only_on_reaching_weles() {
    let fleet = Fleet::new();
    fleet.env_file(&long_allowlist(&["generic_browser_task"]));
    let out = fleet.task(&[]);
    assert!(!out.status.success(), "there is no Weles API in this test");
    let text = said(&out);
    // The gate passed: the failure must not be about the allowlist.
    assert!(!text.contains("does not accept the action"), "{text}");
    assert!(
        !text.contains("declares no WELES_ACTION_ALLOWLIST"),
        "{text}"
    );
    // And it must be recognisably about reaching the service instead.
    assert!(
        text.contains("weles-admission")
            || text.contains("admission")
            || text.contains("directory")
            || text.contains("endpoint"),
        "the failure should be about reaching Weles:\n{text}"
    );
}

#[test]
fn a_url_with_embedded_credentials_is_refused() {
    let fleet = Fleet::new();
    fleet.env_file(&long_allowlist(&["generic_browser_task"]));
    let out = fleet.stado(&[
        "host",
        "weles-browser-task",
        "here",
        "--url",
        "https://user:secret@accounts.google.com/",
        "--objective",
        "x",
        "--session-label",
        "l",
    ]);
    assert!(!out.status.success());
    let text = said(&out);
    assert!(text.contains("without embedded credentials"), "{text}");
    assert!(
        !text.contains("secret"),
        "the refusal must not echo the credential:\n{text}"
    );
}

#[test]
fn an_empty_objective_is_refused() {
    let fleet = Fleet::new();
    fleet.env_file(&long_allowlist(&["generic_browser_task"]));
    let out = fleet.stado(&[
        "host",
        "weles-browser-task",
        "here",
        "--url",
        "https://example.com/",
        "--objective",
        "   ",
        "--session-label",
        "l",
    ]);
    assert!(!out.status.success());
    // A blank objective is only blank after the `@file` trim, so this also
    // pins that an all-whitespace objective is not a task.
    let text = said(&out);
    assert!(
        text.contains("--objective is empty") || text.contains("objective"),
        "{text}"
    );
}

/// A sign-in needs both halves: the origin whose fields are filled and the
/// vault item that holds the account.
#[test]
fn half_a_sign_in_is_refused_naming_the_missing_half() {
    let fleet = Fleet::new();
    fleet.env_file(&long_allowlist(&["generic_browser_task"]));

    let out = fleet.task(&[
        "--allow-login",
        "--sign-in-origin",
        "https://accounts.google.com",
    ]);
    assert!(!out.status.success(), "{}", said(&out));
    let text = said(&out);
    assert!(
        text.contains("--sign-in-origin needs --sign-in-item"),
        "{text}"
    );
    assert!(text.contains("vault item"), "{text}");

    let out = fleet.task(&["--allow-login", "--sign-in-item", "weles-google-sso-login"]);
    assert!(!out.status.success(), "{}", said(&out));
    let text = said(&out);
    assert!(
        text.contains("--sign-in-item needs --sign-in-origin"),
        "{text}"
    );
}

/// Handing an agent credentials while its own instructions say "do not log
/// in" is two orders. This is the one mechanical consequence --allow-login
/// has, and the help text now claims nothing more than that.
#[test]
fn a_sign_in_without_allow_login_is_refused() {
    let fleet = Fleet::new();
    fleet.env_file(&long_allowlist(&["generic_browser_task"]));
    let out = fleet.task(&[
        "--sign-in-origin",
        "https://accounts.google.com",
        "--sign-in-item",
        "weles-google-sso-login",
    ]);
    assert!(!out.status.success(), "{}", said(&out));
    let text = said(&out);
    assert!(text.contains("requires --allow-login"), "{text}");
}

/// Weles builds its expectation from the live page's `origin`, so an origin
/// carrying a path could never match it. Refused before a capability exists,
/// because a minted one is single-use and would be spent finding that out.
#[test]
fn an_origin_weles_could_never_match_is_refused_before_any_host_is_touched() {
    let fleet = Fleet::new();
    fleet.env_file(&long_allowlist(&["generic_browser_task"]));

    let out = fleet.task(&[
        "--allow-login",
        "--sign-in-origin",
        "https://accounts.google.com/signin/v2",
        "--sign-in-item",
        "weles-google-sso-login",
    ]);
    assert!(!out.status.success(), "{}", said(&out));
    let text = said(&out);
    assert!(text.contains("bare origin"), "{text}");

    let out = fleet.task(&[
        "--allow-login",
        "--sign-in-origin",
        "ftp://accounts.google.com",
        "--sign-in-item",
        "weles-google-sso-login",
    ]);
    assert!(!out.status.success(), "{}", said(&out));
    let text = said(&out);
    assert!(
        text.contains("credential fill requires an HTTP(S) origin"),
        "the refusal must be the worker's own sentence:\n{text}"
    );
}

/// The capability must exist in the broker the WORKER talks to, so it is
/// issued on the target over the audited channel. The registry target here
/// names this machine, so that channel runs locally against a tempdir HOME
/// which genuinely holds no `.stado/bin/skarbiec` — nothing is stubbed, and
/// the refusal has to name the host and the path rather than submit a run
/// whose prefill could never be redeemed.
#[test]
fn a_sign_in_is_refused_when_the_target_has_no_capability_broker() {
    let fleet = Fleet::new();
    fleet.env_file(&long_allowlist(&["generic_browser_task"]));
    let out = fleet.task(&[
        "--allow-login",
        "--sign-in-origin",
        "https://accounts.google.com",
        "--sign-in-item",
        "weles-google-sso-login",
    ]);
    assert!(!out.status.success(), "{}", said(&out));
    let text = said(&out);
    assert!(
        text.contains("no Skarbiec binary at"),
        "the refusal must name the broker it looked for:\n{text}"
    );
    assert!(text.contains(".stado/bin/skarbiec"), "{text}");
    assert!(
        text.contains("where it would be redeemed"),
        "the refusal must say why the target is the host that matters:\n{text}"
    );
    assert!(
        text.contains("here"),
        "the refusal must name the host:\n{text}"
    );
}

/// The new verb's own input rules: reading takes only TARGET, declaring takes
/// all four flags. A partial declaration is the one input that could look like
/// a read and write something.
#[test]
fn declaring_a_capability_route_takes_all_four_flags_or_none() {
    let fleet = Fleet::new();

    let out = fleet.stado(&[
        "host",
        "capability-route",
        "here",
        "--resource",
        "origin:https://accounts.google.com/email",
    ]);
    assert!(!out.status.success(), "{}", said(&out));
    let text = said(&out);
    assert!(
        text.contains("--resource, --item, --field and --reason"),
        "{text}"
    );

    let out = fleet.stado(&[
        "host",
        "capability-route",
        "here",
        "--item",
        "weles-google-sso-login",
        "--field",
        "username",
    ]);
    assert!(!out.status.success(), "{}", said(&out));
    let text = said(&out);
    assert!(text.contains("reading takes only TARGET"), "{text}");

    let out = fleet.stado(&[
        "host",
        "capability-route",
        "here",
        "--resource",
        "origin:https://accounts.google.com/email",
        "--item",
        "weles-google-sso-login",
        "--field",
        "username",
        "--reason",
        "   ",
    ]);
    assert!(!out.status.success(), "{}", said(&out));
    let text = said(&out);
    assert!(text.contains("--reason must say why"), "{text}");
}

/// And its read path reaches the target's own broker, refusing in the same
/// words when that host carries none.
#[test]
fn reading_capability_routes_names_the_targets_own_broker() {
    let fleet = Fleet::new();
    let out = fleet.stado(&["host", "capability-route", "here"]);
    assert!(!out.status.success(), "{}", said(&out));
    let text = said(&out);
    assert!(text.contains("no Skarbiec binary at"), "{text}");
    assert!(text.contains(".stado/bin/skarbiec"), "{text}");
}
