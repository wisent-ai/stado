//! Admission by room, not by percentage. On a real APFS volume filled past
//! the janitor's disk-full rule with the user's own data, the real
//! `stado serve --worker` publishes that the rule holds and still claims and
//! runs ordinary work, because the volume's free bytes are above the room work
//! there needs: no release scratch recorded on the host, no janitor margin.
//! Once a release build's scratch larger than the free bytes is kept on the
//! host, as a release worker keeps it, the same worker refuses ordinary work
//! with `disk_room_short` naming the free bytes and the room, the queued job
//! stays queued, and `stado host gates` says the same.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Receiver};

use serde_json::{json, Value};

use super::native::Native;

/// The isolated registry's one host, as [`Native::new`] names it.
const HOST: &str = "example-cleanup-host";

/// Every job document under `prefix/` of the store at `root`.
fn jobs(root: &Path, prefix: &str) -> Vec<Value> {
    let directory = root.join(prefix);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => panic!("{} cannot be listed: {error}", directory.display()),
    };
    entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .filter_map(|path| match fs::read(&path) {
            Ok(bytes) => Some(serde_json::from_slice(&bytes).unwrap()),
            // Moved to its next state between the listing and this read.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!("{} cannot be read: {error}", path.display()),
        })
        .collect()
}

/// Whether a job whose command carries `marker` is under `prefix/`.
fn holds(root: &Path, prefix: &str, marker: &str) -> bool {
    jobs(root, prefix).iter().any(|job| {
        job["command"]
            .as_str()
            .is_some_and(|command| command.contains(marker))
    })
}

/// `stado host gates` for the isolated host, read from stdout: it exits
/// non-zero while any blocker stands.
fn gates(native: &Native) -> Value {
    let output = native.run(&["host", "gates", HOST, "--json"]);
    serde_json::from_slice(&output.stdout).expect("host gates printed JSON")
}

/// Whether `list` holds the word `word`.
fn names(list: &Value, word: &str) -> bool {
    list.as_array()
        .is_some_and(|words| words.iter().any(|said| said == word))
}

/// The real worker on the isolated home, and its log lines as they come,
/// kept in `worker.log` beside the home.
struct Worker {
    child: Child,
    lines: Receiver<String>,
}

impl Worker {
    fn start(native: &Native) -> Self {
        let log = native.home.parent().unwrap().join("worker.log");
        let mut child = native
            .command(&[
                "serve",
                "--worker",
                "--standalone",
                "--kind",
                "local",
                "--poll-seconds",
                "1",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the real worker");
        let stderr = child.stderr.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            let mut file = fs::File::create(log).unwrap();
            for line in BufReader::new(stderr).lines() {
                let line = line.unwrap();
                writeln!(file, "{line}").unwrap();
                // The journey stops reading once it has its verdict.
                let _ = sender.send(line);
            }
        });
        Self { child, lines }
    }

    /// Read log lines until one carries `needle`, returning it.
    fn line_with(&self, needle: &str) -> String {
        for line in self.lines.iter() {
            if line.contains(needle) {
                return line;
            }
        }
        panic!("the worker ended before logging {needle:?}; read worker.log beside the home");
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Err(error) = self.child.kill() {
            eprintln!("cannot stop the worker: {error}");
        }
        if let Err(error) = self.child.wait() {
            eprintln!("cannot reap the worker: {error}");
        }
    }
}

#[test]
fn past_the_rule_a_host_takes_work_that_fits_and_refuses_what_does_not() {
    let native = Native::new("room-admission");
    let storage = native.registry.parent().unwrap().to_path_buf();
    native.fill_with_user_data();
    // Declared after the volume, so it is stopped before the volume detaches.
    let worker = Worker::start(&native);

    // Past the rule, with room: the worker accepts once its janitor has
    // reported a margin, and the gates read the rule as a note.
    let accepting = worker.line_with("admission-measured: published accepting_jobs=true");
    native.observe("accepting past the rule", json!(accepting));
    let fitting = gates(&native);
    native.observe("gates with room", fitting.clone());
    assert_eq!(fitting["disk"]["full"], true, "{fitting}");
    assert_eq!(fitting["disk"]["room"]["fits"], true, "{fitting}");
    assert!(
        names(&fitting["notes"], "disk_pressure_active"),
        "{fitting}"
    );
    for word in ["disk_room_short", "disk_pressure_active"] {
        assert!(
            !names(&fitting["blockers"], word),
            "{word} blocks: {fitting}"
        );
    }
    native.success(&["submit", "--run-id", "room-fits", "echo room-fits"]);
    for line in worker.lines.iter() {
        assert!(
            !holds(&storage, "failed", "room-fits"),
            "the admitted job failed; read worker.log: {line}"
        );
        if holds(&storage, "completed", "room-fits") {
            break;
        }
    }
    assert!(
        holds(&storage, "completed", "room-fits"),
        "the worker ended before the admitted job completed; read worker.log"
    );
    native.observe(
        "completed past the rule",
        json!(jobs(&storage, "completed")),
    );

    // A release scratch larger than the free bytes, kept as a release worker
    // on this host keeps it: the room no longer fits.
    let free = fs2::available_space(&native.home).unwrap();
    let receipt = stado::release_pipeline::ScratchReceipt {
        schema_version: stado::release_pipeline::SCHEMA_VERSION,
        run_id: "room-admission".to_string(),
        job_id: "room-admission-build".to_string(),
        product: "stado".to_string(),
        platform: "darwin-arm64".to_string(),
        builder: HOST.to_string(),
        bytes: fs2::total_space(&native.home).unwrap(),
        free_bytes: free,
        build: stado::release_pipeline::StepStatus::Passed,
        measured_at: chrono::Utc::now().to_rfc3339(),
    };
    let since = chrono::Utc::now()
        - chrono::Duration::days(stado::providers::local::space_reclaim::margin::RECENT_DAYS);
    stado::release_pipeline::record_on_host(&native.state_dir(), &receipt, since)
        .expect("keep the scratch as a release worker does");
    let refused = worker.line_with("loop: disk_room_short:");
    native.observe("refused short of room", json!(refused));
    for figure in [
        "GiB free against",
        "GiB needed",
        "the stado darwin-arm64 build of run room-admission",
    ] {
        assert!(
            refused.contains(figure),
            "the refusal names {figure:?}: {refused}"
        );
    }

    // Ordinary work submitted now stays queued through two more refusals.
    native.success(&["submit", "--run-id", "room-short", "echo room-short"]);
    worker.line_with("loop: disk_room_short:");
    worker.line_with("loop: disk_room_short:");
    assert!(
        holds(&storage, "queue", "room-short"),
        "the job left the queue"
    );
    for state in ["running", "completed", "failed"] {
        assert!(
            !holds(&storage, state, "room-short"),
            "a job short of room reached {state}"
        );
    }
    let short = gates(&native);
    native.observe("gates short of room", short.clone());
    assert!(names(&short["blockers"], "disk_room_short"), "{short}");
    assert_eq!(short["disk"]["room"]["fits"], false, "{short}");
    assert_eq!(
        short["capacity"]["admission_reason"], "disk_room_short",
        "{short}"
    );
    let Some(detail) = short["capacity"]["admission_detail"].as_str() else {
        panic!("host gates carries no admission detail: {short}");
    };
    assert!(
        detail.contains("GiB free against") && detail.contains("GiB needed"),
        "host gates names both figures: {short}"
    );
}
