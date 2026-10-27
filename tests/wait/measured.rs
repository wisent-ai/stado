//! A wait that ended is measured, and the next wait on the same operation
//! says when it is due.
//!
//! The real `stado registry host show` runs twice against an isolated local
//! store under this checkout's build directory, nothing holding its registry
//! lock, with a measurement directory of the test's own. The first run's
//! `czekam` line on the lock says `termin: brak` and why: nothing on that
//! operation has ended here. After it, the measurement directory holds the
//! operation's longest wait. The second run's line carries `termin:` as an
//! RFC 3339 time and `pomiar:` naming the seconds and the same operation. A
//! measurement file that holds no seconds is said in `pomiar` with its path.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn root(case: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../.build/journeys/wait-measured")
        .join(format!("{case}-{}", std::process::id()));
    for directory in [".locks", ".metadata"] {
        std::fs::create_dir_all(root.join("store").join(directory))
            .expect("lay out the isolated local store");
    }
    root
}

fn show(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["registry", "host", "show", "example-host", "--path", "kind"])
        .current_dir(root)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", root.join("store"))
        .env("STADO_CONFIG", root.join("no-config.toml"))
        .env("STADO_WAITS_DIR", root.join("waits"))
        .env("STADO_WAITS_MEASURED_DIR", root.join("measured"))
        .stdin(Stdio::null())
        .output()
        .expect("run the real stado")
}

/// The `czekam` lines of `output` on the registry lock.
fn lock_lines(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter(|line| line.starts_with("czekam: ") && line.contains("rodzaj: blokada"))
        .map(str::to_owned)
        .collect()
}

/// The `termin:` field of a `czekam` line.
fn termin(line: &str) -> &str {
    line.split("; ")
        .find_map(|field| field.strip_prefix("termin: "))
        .expect("a termin field")
}

#[test]
fn a_wait_that_ended_makes_the_next_one_say_when_it_is_due() {
    let root = root("due");
    let first = show(&root);
    let said = String::from_utf8_lossy(&first.stderr).into_owned();
    let opened = lock_lines(&first);
    assert!(
        !opened.is_empty(),
        "the first run opened no lock wait: {said}"
    );
    for line in &opened {
        assert_eq!(
            termin(line),
            "brak",
            "the first wait has nothing to be due by: {line}"
        );
        assert!(
            line.contains("nie skonczylo sie tu"),
            "pomiar says why: {line}"
        );
    }
    let measured: Vec<PathBuf> = std::fs::read_dir(root.join("measured"))
        .expect("the ended wait was measured")
        .map(|entry| entry.expect("a measurement").path())
        .collect();
    assert!(!measured.is_empty(), "no measurement was kept");
    let records: Vec<&PathBuf> = measured
        .iter()
        .filter(|file| file.extension().is_none())
        .collect();
    assert!(
        !records.is_empty(),
        "no record beside the lock files: {measured:?}"
    );
    for record in &records {
        let name = record
            .file_name()
            .expect("a record's name")
            .to_string_lossy()
            .into_owned();
        let digest = name.chars().all(|character| character.is_ascii_hexdigit());
        assert!(
            digest,
            "a record is not named by its operation's digest: {name}"
        );
        let kept = std::fs::read_to_string(record).expect("read a record");
        let (nanos, operation) = kept
            .trim()
            .split_once(' ')
            .expect("nanoseconds, then the operation");
        nanos
            .parse::<u64>()
            .expect("the record holds whole nanoseconds");
        assert!(
            operation.starts_with("blokada "),
            "the record names its operation: {kept}"
        );
    }

    let second = show(&root);
    let said = String::from_utf8_lossy(&second.stderr).into_owned();
    let due = lock_lines(&second);
    assert!(
        !due.is_empty(),
        "the second run opened no lock wait: {said}"
    );
    for line in &due {
        chrono::DateTime::parse_from_rfc3339(termin(line)).expect("termin is an RFC 3339 time");
        assert!(
            line.contains("s, blokada "),
            "pomiar names seconds and the operation: {line}"
        );
    }

    for file in &measured {
        std::fs::write(file, "no seconds\n").expect("spoil the measurement");
    }
    let third = show(&root);
    for line in &lock_lines(&third) {
        assert_eq!(
            termin(line),
            "brak",
            "an unreadable measurement makes no due time: {line}"
        );
        assert!(
            line.contains("not nanoseconds"),
            "pomiar names the unreadable file: {line}"
        );
    }
}
