//! `stado service env-show` / `service endpoint-check` against a real env file
//! and a real listening socket.
//!
//! Every test drives the built `stado` binary. The registry target's
//! `hostnames` name THIS machine, so `deploy/host_channel.rs` runs the remote
//! script locally through the same `/bin/bash -s` the ssh branch asks the login
//! shell for — the script under test is byte-identical either way, and only the
//! hop disappears. HOME is a tempdir, so the file being read, the symlink being
//! refused and the socket being reconciled are all real state this test made,
//! and the operator's own `~/.config` can never be reached.
//!
//! There is no fake Skarbiec here and no stub socket table. The "live
//! endpoint" is a `TcpListener` this test binds on loopback, and the process
//! `endpoint-check` names as holding it is this test process.
//!
//! What is defended: a duplicate key is reported twice in file order with the
//! winner named; a credential-shaped value never crosses the channel while an
//! endpoint-shaped one does whatever its key is called; `--reveal` opens
//! exactly one key; a symlink and a path outside the target home are refused
//! with their exact sentences; and `endpoint-check` exits non-zero for a
//! loopback port nothing is listening on, judging the effective assignment
//! rather than the shadowed one.

use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output};

/// The service label every test addresses. Declared on the target itself, the
/// way `deploy/service.rs::declared_services` reads a registry-managed unit.
const SERVICE: &str = "com.wisent.always-on.weles";

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
                "services": [{
                    "label": SERVICE,
                    "name": SERVICE,
                    "kind": "launchd",
                    "path": format!("/Library/LaunchDaemons/{SERVICE}.plist"),
                    "program": "/bin/sh"
                }]
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

    /// Write the env file the tests read, and return its absolute path.
    fn env_file(&self, body: &str) -> PathBuf {
        let directory = self.home.path().join(".config/weles");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("worker.env");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(body.as_bytes()).unwrap();
        drop(file);
        // 0600, the mode `env-set` leaves behind, so `owner_only` is exercised
        // as the true state of a real file rather than asserted about nothing.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        path
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

    fn env_show(&self, env_file: &str, extra: &[&str]) -> Output {
        let mut args = vec![
            "service",
            "env-show",
            SERVICE,
            "--host",
            "here",
            "--env-file",
            env_file,
        ];
        args.extend_from_slice(extra);
        self.stado(&args)
    }

    fn endpoint_check(&self, env_file: &str, extra: &[&str]) -> Output {
        let mut args = vec![
            "service",
            "endpoint-check",
            SERVICE,
            "--host",
            "here",
            "--env-file",
            env_file,
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

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The row for one key, as the printed table spells it.
fn row<'a>(text: &'a str, key: &str) -> &'a str {
    text.lines()
        .find(|line| line.split_whitespace().nth(2) == Some(key))
        .unwrap_or_else(|| panic!("no table row for {key} in:\n{text}"))
}

#[test]
fn env_show_reports_a_duplicate_key_in_file_order_and_names_the_winner() {
    let fleet = Fleet::new();
    // The shape the 2026-08-30 outage had: the value an operator wrote with
    // `env-set` sits above an `export` spelling of the same variable, which
    // `env-set`'s `^KEY=` rewrite cannot see and which wins when sourced.
    let path = fleet.env_file(
        "# nonsecret worker configuration\n\
         WC_SKARBIEC_URL=http://127.0.0.1:8895\n\
         WELES_QUEUE=default\n\
         export WC_SKARBIEC_URL=http://127.0.0.1:8785\n",
    );

    let out = fleet.env_show(path.to_str().unwrap(), &[]);
    assert!(out.status.success(), "env-show failed: {}", stderr(&out));
    let text = stdout(&out);

    // Both assignments are listed, on their own lines, in file order.
    let first = text
        .find("http://127.0.0.1:8895")
        .expect("first value shown");
    let second = text
        .find("http://127.0.0.1:8785")
        .expect("second value shown");
    assert!(first < second, "assignments out of file order:\n{text}");
    // Each assignment carries its own line number and the form it was written
    // in. The `export` spelling is reported as itself rather than normalized
    // into the plain one, because that is the difference `env-set` is blind to.
    let assignments: Vec<Vec<&str>> = text
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<&str>>())
        .filter(|fields| fields.get(2) == Some(&"WC_SKARBIEC_URL"))
        .collect();
    assert_eq!(
        assignments.len(),
        2,
        "both assignments are not listed:\n{text}"
    );
    // LINE FORM KEY RESOLUTION VALUE_STATE CHARS VALUE
    assert_eq!(assignments[0][0], "2", "wrong line number:\n{text}");
    assert_eq!(assignments[0][1], "assignment", "wrong form:\n{text}");
    assert_eq!(assignments[0][3], "shadowed", "wrong resolution:\n{text}");
    assert_eq!(assignments[1][0], "4", "wrong line number:\n{text}");
    assert_eq!(assignments[1][1], "export", "wrong form:\n{text}");
    assert_eq!(assignments[1][3], "effective", "wrong resolution:\n{text}");
    assert!(
        text.contains("duplicates: WC_SKARBIEC_URL"),
        "the duplicate is not called out:\n{text}"
    );
    assert!(
        text.contains("the LAST assignment wins when this file is sourced"),
        "the duplicate note does not say which one wins:\n{text}"
    );
    // The env file's real mode is reported beside its contents.
    assert!(
        text.contains("mode 600, owner-only"),
        "file protection not reported:\n{text}"
    );
}

#[test]
fn env_show_says_every_key_is_assigned_once_when_none_repeats() {
    let fleet = Fleet::new();
    let path = fleet.env_file("WELES_QUEUE=default\nWC_SKARBIEC_URL=http://127.0.0.1:8895\n");

    let out = fleet.env_show(path.to_str().unwrap(), &[]);
    assert!(out.status.success(), "env-show failed: {}", stderr(&out));
    assert!(
        stdout(&out).contains("duplicates: none — every key is assigned exactly once"),
        "got:\n{}",
        stdout(&out)
    );
}

#[test]
fn env_show_withholds_credentials_and_shows_endpoints_whatever_the_key_is_called() {
    let fleet = Fleet::new();
    let path = fleet.env_file(
        "WELES_API_TOKEN=super-secret-bearer-value\n\
         WELES_CREDENTIAL_SKARBIEC_URL=http://127.0.0.1:8895\n\
         WELES_DATABASE_URL=postgres://weles:hunter2@db.internal:5432/weles\n\
         WELES_API_PORT=8896\n\
         WELES_STATE_DIR=$HOME/.local/state/weles\n",
    );

    let out = fleet.env_show(path.to_str().unwrap(), &[]);
    assert!(out.status.success(), "env-show failed: {}", stderr(&out));
    let text = stdout(&out);

    // A credential-shaped key never puts its value on the wire, and its
    // length is reported so the operator still learns something.
    assert!(
        !text.contains("super-secret-bearer-value"),
        "a secret crossed the channel:\n{text}"
    );
    assert!(
        row(&text, "WELES_API_TOKEN").contains("redacted"),
        "the token is not marked redacted:\n{text}"
    );
    assert!(
        row(&text, "WELES_API_TOKEN").contains("25"),
        "the withheld value's length is not reported:\n{text}"
    );
    // The key an operator must verify carries CREDENTIAL in its name and is
    // shown anyway, because its value is an inert endpoint.
    assert!(
        row(&text, "WELES_CREDENTIAL_SKARBIEC_URL").contains("http://127.0.0.1:8895"),
        "an endpoint was hidden behind its key name:\n{text}"
    );
    // A URL carrying userinfo is withheld even though its key names no secret.
    assert!(
        !text.contains("hunter2"),
        "a URL password crossed the channel:\n{text}"
    );
    assert!(
        row(&text, "WELES_DATABASE_URL").contains("redacted"),
        "a userinfo URL is not marked redacted:\n{text}"
    );
    assert!(
        row(&text, "WELES_API_PORT").contains("8896"),
        "a port was hidden:\n{text}"
    );
    // A reference to another variable is not a secret and is shown.
    assert!(
        row(&text, "WELES_STATE_DIR").contains("$HOME/.local/state/weles"),
        "a variable reference was hidden:\n{text}"
    );
    assert!(
        text.contains("redacted: 2 value(s) never left the host"),
        "the withheld count is not reported:\n{text}"
    );
}

#[test]
fn env_show_reveals_exactly_the_one_key_named() {
    let fleet = Fleet::new();
    let path = fleet.env_file(
        "WELES_API_TOKEN=super-secret-bearer-value\n\
         WELES_OTHER_TOKEN=another-secret-value\n",
    );

    let out = fleet.env_show(path.to_str().unwrap(), &["--reveal", "WELES_API_TOKEN"]);
    assert!(out.status.success(), "env-show failed: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("super-secret-bearer-value"),
        "the revealed key was still withheld:\n{text}"
    );
    assert!(
        row(&text, "WELES_API_TOKEN").contains("revealed"),
        "the revealed key is not marked as such:\n{text}"
    );
    assert!(
        !text.contains("another-secret-value"),
        "--reveal opened a key it was not given:\n{text}"
    );
}

#[test]
fn env_show_refuses_a_key_that_is_not_an_environment_variable_name() {
    let fleet = Fleet::new();
    let path = fleet.env_file("WELES_QUEUE=default\n");

    let out = fleet.env_show(path.to_str().unwrap(), &["--reveal", "weles_queue"]);
    assert!(!out.status.success(), "env-show accepted a lowercase key");
    assert!(
        stderr(&out).contains("--key must be an uppercase environment variable name"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn env_show_refuses_a_symlink_without_following_it() {
    let fleet = Fleet::new();
    let real = fleet.env_file("WELES_QUEUE=default\n");
    let link = fleet.home.path().join(".config/weles/worker.env.link");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let out = fleet.env_show(link.to_str().unwrap(), &[]);
    assert!(!out.status.success(), "env-show followed a symlink");
    assert!(
        stderr(&out).contains("refused_symlink — the target is a symlink and was not followed"),
        "got: {}",
        stderr(&out)
    );
    assert!(
        !stdout(&out).contains("WELES_QUEUE"),
        "the symlink's target was read anyway:\n{}",
        stdout(&out)
    );
}

#[test]
fn env_show_refuses_a_path_outside_the_target_home() {
    let fleet = Fleet::new();
    fleet.env_file("WELES_QUEUE=default\n");

    let out = fleet.env_show("/etc/hosts", &[]);
    assert!(!out.status.success(), "env-show read outside the home");
    assert!(
        stderr(&out).contains("refused_outside_home — the target must be inside the target home"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn env_show_reports_a_file_that_is_not_there_rather_than_an_empty_one() {
    let fleet = Fleet::new();
    let missing = fleet.home.path().join(".config/weles/absent.env");

    let out = fleet.env_show(missing.to_str().unwrap(), &[]);
    assert!(!out.status.success(), "env-show invented a file");
    assert!(
        stderr(&out).contains("missing — no regular file at the target"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn env_show_reports_a_line_that_is_not_an_assignment() {
    let fleet = Fleet::new();
    // A sourced second file changes what the whole env file means, and is
    // exactly the kind of line a reader that collapsed the file into a map
    // would drop.
    let path = fleet.env_file("WELES_QUEUE=default\n. $HOME/.config/weles/extra.env\n");

    let out = fleet.env_show(path.to_str().unwrap(), &[]);
    assert!(out.status.success(), "env-show failed: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("unparsable"),
        "a non-assignment line is not reported:\n{text}"
    );
    assert!(
        text.contains(". $HOME/.config/weles/extra.env"),
        "the non-assignment line's text is not shown:\n{text}"
    );
}

#[test]
fn endpoint_check_names_the_process_holding_a_live_loopback_port() {
    let fleet = Fleet::new();
    // A real socket, held by this test process for the duration of the check.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let live = listener.local_addr().unwrap().port();
    let path = fleet.env_file(&format!(
        "WC_SKARBIEC_URL=http://127.0.0.1:{live}\nWELES_QUEUE=default\n"
    ));

    let out = fleet.endpoint_check(path.to_str().unwrap(), &[]);
    assert!(
        out.status.success(),
        "endpoint-check failed on a live endpoint: {}",
        stderr(&out)
    );
    let text = stdout(&out);
    let line = text
        .lines()
        .find(|line| line.starts_with("WC_SKARBIEC_URL"))
        .unwrap_or_else(|| panic!("no endpoint row:\n{text}"));
    assert!(
        line.contains("listening"),
        "a held port is not reported as listening:\n{text}"
    );
    assert!(
        line.contains(&format!("{live}")) && line.contains("pid"),
        "the holding process is not named:\n{text}"
    );
    // A value that is not an endpoint is not invented into one.
    assert!(
        !text.contains("WELES_QUEUE"),
        "a non-endpoint value was reconciled:\n{text}"
    );
    drop(listener);
}

#[test]
fn endpoint_check_fails_when_a_declared_loopback_endpoint_is_dead() {
    let fleet = Fleet::new();
    // A port this test held and released: nothing is listening there now, and
    // the kernel will not have handed it out again inside one test.
    let dead = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let path = fleet.env_file(&format!("WC_SKARBIEC_URL=http://127.0.0.1:{dead}\n"));

    let out = fleet.endpoint_check(path.to_str().unwrap(), &[]);
    assert!(
        !out.status.success(),
        "endpoint-check passed a dead dependency:\n{}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("nothing is listening where WC_SKARBIEC_URL points"),
        "got: {}",
        stderr(&out)
    );
    assert!(
        stdout(&out)
            .lines()
            .any(|line| line.starts_with("WC_SKARBIEC_URL") && line.contains("dead")),
        "the dead endpoint is not in the table:\n{}",
        stdout(&out)
    );
}

#[test]
fn endpoint_check_judges_the_effective_assignment_not_the_shadowed_one() {
    let fleet = Fleet::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let live = listener.local_addr().unwrap().port();
    let dead = {
        let scratch = TcpListener::bind("127.0.0.1:0").unwrap();
        scratch.local_addr().unwrap().port()
    };
    // The live endpoint first, the dead one second: a reader that took the
    // first assignment, or that collapsed the file into a map by insertion
    // order, would report this file as healthy.
    let path = fleet.env_file(&format!(
        "WC_SKARBIEC_URL=http://127.0.0.1:{live}\nexport WC_SKARBIEC_URL=http://127.0.0.1:{dead}\n"
    ));

    let out = fleet.endpoint_check(path.to_str().unwrap(), &[]);
    assert!(
        !out.status.success(),
        "endpoint-check judged the shadowed assignment:\n{}",
        stdout(&out)
    );
    let text = stdout(&out);
    let rows: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("WC_SKARBIEC_URL"))
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "a shadowed assignment was reconciled too:\n{text}"
    );
    assert!(
        rows[0].contains(&format!("{dead}")) && rows[0].contains("dead"),
        "the effective assignment is not the one judged:\n{text}"
    );
    assert!(
        text.contains("duplicates: WC_SKARBIEC_URL"),
        "the shadowing that caused this is not reported:\n{text}"
    );
    drop(listener);
}

#[test]
fn endpoint_check_does_not_judge_a_remote_endpoint_against_this_host() {
    let fleet = Fleet::new();
    let path = fleet.env_file("WELES_UPSTREAM_URL=https://api.example.com/v1\n");

    let out = fleet.endpoint_check(path.to_str().unwrap(), &[]);
    assert!(
        out.status.success(),
        "a remote endpoint was judged against this host's sockets: {}",
        stderr(&out)
    );
    assert!(
        stdout(&out)
            .lines()
            .any(|line| line.starts_with("WELES_UPSTREAM_URL") && line.contains("remote")),
        "got:\n{}",
        stdout(&out)
    );
}
