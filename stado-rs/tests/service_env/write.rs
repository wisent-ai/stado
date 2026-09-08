//! `service env-set`: one key written into a real file, and read back.
//!
//! The write goes to a real env file under a tempdir HOME and every case
//! finishes by reading that file off disk: the new assignment is there, the
//! key the test did not name is untouched, and the file is still owner-only.
//!
//! The reverting case runs a real competing writer, shaped exactly like the
//! one this read-back check was built for. On charless-mac-mini
//! `$HOME/.stado/bin/weles-release-cutover` deletes `^WC_SKARBIEC_URL=` from
//! the worker env file and appends the contents of
//! `$HOME/.stado/forwards/skarbiec.url` back in. This is that, in a loop: a
//! real process performing real atomic replacements of the real file under
//! test, which is the condition `env-set`'s read-back exists to detect.

use std::path::PathBuf;
use std::process::Command;

use crate::{on_disk, set_mode, stderr, stdout, Fleet, OWNER_ONLY};

/// A real forward marker, the way `host forward-local` leaves one.
fn marker(fleet: &Fleet, name: &str, url: &str) -> PathBuf {
    let directory = fleet.home.path().join(".stado/forwards");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{name}.url"));
    std::fs::write(&path, format!("{url}\n")).unwrap();
    set_mode(&path, OWNER_ONLY);
    path
}

struct Reconciler {
    child: std::process::Child,
    stop: PathBuf,
}

impl Reconciler {
    fn start(fleet: &Fleet, env_file: &PathBuf) -> Self {
        let stop = fleet.storage.path().join("reconciler.stop");
        let script = fleet.storage.path().join("reconcile.sh");
        std::fs::write(
            &script,
            r#"#!/bin/bash
set -eu
umask 077
while [ ! -f "$STOP" ]; do
  IFS= read -r url < "$HOME/.stado/forwards/skarbiec.url" || continue
  body=""
  while IFS= read -r line; do
    case "$line" in WC_SKARBIEC_URL=*) continue ;; esac
    body="$body$line
"
  done < "$ENV_FILE"
  tmp="$ENV_FILE.reconcile"
  printf '%s' "$body" > "$tmp"
  printf "WC_SKARBIEC_URL='%s'\n" "$url" >> "$tmp"
  /bin/mv -f "$tmp" "$ENV_FILE"
done
"#,
        )
        .unwrap();
        let child = Command::new("bash")
            .arg(&script)
            .env("HOME", fleet.home.path())
            .env("ENV_FILE", env_file)
            .env("STOP", &stop)
            .spawn()
            .expect("reconciler starts");
        // Let it take ownership of the file at least once before the write
        // under test, so the test is measuring detection and not a startup
        // race.
        std::thread::sleep(std::time::Duration::from_millis(200));
        Self { child, stop }
    }
}

impl Drop for Reconciler {
    fn drop(&mut self) {
        std::fs::write(&self.stop, "stop").ok();
        let _ = self.child.wait();
    }
}

/// The file's mode as it stands on disk, in the spelling the product prints.
fn mode_on_disk(path: &std::path::Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).unwrap().permissions().mode();
        return format!("{:o}", mode & 0o7777);
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        String::new()
    }
}

#[test]
fn env_set_confirms_a_write_that_survived() {
    let fleet = Fleet::new();
    let path = fleet.env_file("WELES_QUEUE=default\nWC_SKARBIEC_URL=http://127.0.0.1:8785\n");

    let out = fleet.env_set(
        "WC_SKARBIEC_URL",
        path.to_str().unwrap(),
        "http://127.0.0.1:8895",
    );
    assert!(out.status.success(), "env-set failed: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("matched"),
        "the write was not read back:\n{text}"
    );
    // The write is real: the file on disk holds it, the old assignment is
    // gone, the key nobody named survived, and the file is still owner-only.
    let body = on_disk(&path);
    assert!(
        body.contains("WC_SKARBIEC_URL=http://127.0.0.1:8895"),
        "the new value is not in the file:\n{body}"
    );
    assert!(
        !body.contains("http://127.0.0.1:8785"),
        "the replaced assignment is still in the file:\n{body}"
    );
    assert!(
        body.contains("WELES_QUEUE=default"),
        "a key the write did not name was lost:\n{body}"
    );
    assert_eq!(
        mode_on_disk(&path),
        "600",
        "the write left the file readable beyond its owner"
    );
    // And what the file holds is what a fresh read reports.
    let shown = stdout(&fleet.env_show(path.to_str().unwrap(), &[]));
    assert!(
        shown.contains("http://127.0.0.1:8895") && shown.contains(&format!("{} bytes", body.len())),
        "the read does not agree with the file the write left:\n{shown}"
    );
}

#[test]
fn env_set_verifies_a_withheld_value_without_showing_it() {
    let fleet = Fleet::new();
    let path = fleet.env_file("WELES_QUEUE=default\n");

    let out = fleet.env_set(
        "WELES_API_TOKEN",
        path.to_str().unwrap(),
        "super-secret-bearer-value",
    );
    assert!(out.status.success(), "env-set failed: {}", stderr(&out));
    let text = stdout(&out);
    // Verified exactly — the comparison happened on the host — and the value
    // never came back to be printed.
    assert!(
        text.contains("matched"),
        "a secret write was not verified:\n{text}"
    );
    assert!(
        !text.contains("super-secret-bearer-value"),
        "the read-back printed a secret:\n{text}"
    );
    let body = on_disk(&path);
    assert!(
        body.contains("WELES_API_TOKEN=super-secret-bearer-value"),
        "the write did not land:\n{body}"
    );
    assert_eq!(
        mode_on_disk(&path),
        "600",
        "a secret was written into a file others can read"
    );
}

#[test]
fn env_set_fails_and_names_the_marker_when_a_reconciler_reverts_the_key() {
    let fleet = Fleet::new();
    // The marker the host-side reconciler reads, and the value it will keep
    // forcing back into the file.
    marker(&fleet, "skarbiec", "http://127.0.0.1:8785");
    let path = fleet.env_file("WELES_QUEUE=default\n");
    let _reconciler = Reconciler::start(&fleet, &path);

    let out = fleet.env_set(
        "WC_SKARBIEC_URL",
        path.to_str().unwrap(),
        "http://127.0.0.1:8895",
    );
    assert!(
        !out.status.success(),
        "env-set reported success for a write that was reverted:\nstdout:\n{}\nstderr:\n{}",
        stdout(&out),
        stderr(&out)
    );
    let error = stderr(&out);
    assert!(
        error.contains("WC_SKARBIEC_URL was replaced after the write"),
        "the revert is not reported: {error}"
    );
    assert!(
        error.contains("http://127.0.0.1:8785"),
        "the value that won is not reported: {error}"
    );
    // The whole point: point the operator at the declaration, not the file.
    assert!(
        error.contains("forward marker skarbiec ($HOME/.stado/forwards/skarbiec.url)"),
        "the owning marker is not named: {error}"
    );
    assert!(
        error.contains("correct that marker, not this file"),
        "the operator is not told what to do instead: {error}"
    );
    assert!(
        stdout(&out).contains("differs"),
        "the table does not carry the read-back verdict:\n{}",
        stdout(&out)
    );
    // The refusal is the truth about the file: the reconciler's value is what
    // it holds, and the operator's is nowhere in it.
    let body = on_disk(&path);
    assert!(
        body.contains("WC_SKARBIEC_URL='http://127.0.0.1:8785'"),
        "the reverting writer did not win after all:\n{body}"
    );
    assert!(
        !body.contains("8895"),
        "the value env-set reported as lost is in the file:\n{body}"
    );
}
