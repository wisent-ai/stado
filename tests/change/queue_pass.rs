//! The submit queue advances with no session watching it: every candidate
//! that has no verdict says what it waits on and the command that unblocks
//! it, a candidate whose pass was killed is named as left by a pass that is
//! gone, and `stado change run` with no checkout — the pass the resident
//! host routine runs — resumes it from the checkout it was submitted from.
//!
//! Real Git over a bare repository and a checkout under the repository's
//! ignored `.build/`, the real `stado` binary, and a real pass killed with
//! SIGKILL while it waits on its remote. The remote is made to fail, then to
//! hang, by the checkout's own `core.sshCommand` over an `ssh://` origin;
//! the resumed pass reads the bare repository again and reaches a verdict
//! (the candidate does not merge onto the `main` that moved meanwhile), so
//! no gate, Oko admission or reviewer is needed for the verdict.

use serde_json::{json, Value};
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const OWNER: &str = "change-queue-test";
const NAME: &str = "product";
/// A product of the compiled catalog with a `submit` declaration, so the
/// queue classifies the candidate from the catalog this binary carries.
const PRODUCT: &str = "trends";
const DEFECT: &str = "00000000-0000-4000-8000-00000000c0de";

struct Journey {
    root: PathBuf,
    checkout: PathBuf,
    remote: PathBuf,
    report: Value,
}

impl Journey {
    fn start() -> Self {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let root = repository
            .join(".build/change-queue-pass")
            .join(uuid::Uuid::new_v4().to_string());
        let remote = root.join("remote").join(OWNER).join(format!("{NAME}.git"));
        let checkout = root.join("checkout");
        for directory in [
            &remote,
            &checkout,
            &root.join("home"),
            &root.join("workspace"),
        ] {
            fs::create_dir_all(directory).unwrap();
        }
        fs::create_dir_all(root.join("tmp")).unwrap();
        let journey = Self {
            root,
            checkout,
            remote,
            report: json!({
                "journey": "change-queue-pass",
                "source_revision": env!("STADO_SOURCE_REVISION"),
                "commands": [],
                "outcome": "failed",
            }),
        };
        journey.save();
        journey
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }

    fn record(&mut self, args: &[&str], output: &Output) {
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
    }

    fn git(&mut self, args: &[&str]) -> String {
        // The fixture's own repositories only: a Git variable the caller
        // inherited (an index, a repository) would point git elsewhere.
        let output = Command::new("git")
            .current_dir(&self.checkout)
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", self.root.join("home"))
            .args(args)
            .output()
            .unwrap();
        let mut recorded = vec!["git"];
        recorded.extend(args);
        self.record(&recorded, &output);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn stado(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .current_dir(&self.checkout)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", self.root.join("home"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("STADO_CONFIG", self.root.join("home/.stado/config.json"))
            .env("STADO_CHANGE_QUEUE", self.root.join("queue"))
            .env("WISENT_WORKSPACE", self.root.join("workspace"))
            .stdin(Stdio::null());
        command
    }

    fn run(&mut self, args: &[&str]) -> (bool, String, String) {
        let output = self.stado().args(args).output().unwrap();
        let mut recorded = vec!["stado"];
        recorded.extend(args);
        self.record(&recorded, &output);
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    /// `stado args` run as agent session `session` names it to Stado: the
    /// harness's transcript, opening with that session's record, named by
    /// `PI_SESSION_FILE`, as OMP names it in every command it runs.
    fn run_in_session(&mut self, session: &str, args: &[&str]) -> (bool, String, String) {
        let transcript = self.root.join(format!("{session}.jsonl"));
        fs::write(
            &transcript,
            format!(
                "{}\n",
                json!({"type": "session", "version": 3, "id": session})
            ),
        )
        .unwrap();
        let output = self
            .stado()
            .env("PI_SESSION_FILE", &transcript)
            .args(args)
            .output()
            .unwrap();
        let mut recorded = vec!["stado"];
        recorded.extend(args);
        self.record(&recorded, &output);
        self.report["commands"]
            .as_array_mut()
            .unwrap()
            .last_mut()
            .unwrap()["session"] = json!(session);
        self.save();
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    fn candidate(&self, id: &str) -> Value {
        let path = self
            .root
            .join("queue")
            .join(OWNER)
            .join(NAME)
            .join(format!("{id}.json"));
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap()
    }

    fn origin(&mut self, url: &str, ssh: Option<&str>) {
        let checkout = self.checkout.to_string_lossy().into_owned();
        self.git(&["-C", &checkout, "remote", "set-url", "origin", url]);
        match ssh {
            Some(command) => {
                self.git(&["config", "core.sshCommand", command]);
            }
            None => {
                self.git(&["config", "--unset-all", "core.sshCommand"]);
            }
        }
    }
}

fn commit(journey: &mut Journey, file: &str, text: &str, message: &str) -> String {
    fs::write(journey.checkout.join(file), text).unwrap();
    journey.git(&["add", file]);
    journey.git(&["commit", "--quiet", "-m", message]);
    journey.git(&["rev-parse", "HEAD"])
}

/// A bare `origin` holding `main` with the product's manifest declaring
/// `quality`, and a checkout of it; `main`'s commit.
fn repository(journey: &mut Journey, quality: Value) -> String {
    let remote = journey.remote.to_string_lossy().into_owned();
    journey.git(&[
        "init",
        "--quiet",
        "--bare",
        "--initial-branch=main",
        &remote,
    ]);
    journey.git(&["init", "--quiet", "--initial-branch=main"]);
    journey.git(&["config", "user.name", "Queue Journey"]);
    journey.git(&["config", "user.email", "queue-journey@example.invalid"]);
    journey.git(&["remote", "add", "origin", &remote]);
    let schema: Value = serde_json::from_slice(
        &fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.wisent-release.json")).unwrap(),
    )
    .unwrap();
    let manifest = serde_json::to_string_pretty(&json!({
        "schema_version": schema["schema_version"],
        "product": PRODUCT,
        "releases": false,
        "reason": "A fixture of the submit queue's journey; it publishes no release.",
        "quality": quality,
    }))
    .unwrap();
    commit(
        journey,
        ".wisent-release.json",
        &manifest,
        "declare the product",
    );
    commit(journey, "notes.txt", "one\n", "the base");
    journey.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    journey.git(&["rev-parse", "HEAD"])
}

#[test]
fn a_candidate_whose_pass_was_killed_is_named_and_resumed_by_a_pass_with_no_checkout() {
    let mut journey = Journey::start();
    let remote = journey.remote.to_string_lossy().into_owned();
    let base = repository(&mut journey, json!([]));
    let candidate = commit(&mut journey, "notes.txt", "candidate\n", "the candidate");

    let checkout = journey.checkout.to_string_lossy().into_owned();
    let (submitted, stdout, stderr) = journey.run(&[
        "change", "submit", &checkout, "--commit", &candidate, "--defect", DEFECT, "--json",
    ]);
    assert!(submitted, "submit: {stdout}{stderr}");
    let id = serde_json::from_str::<Value>(&stdout).unwrap()["candidates"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Queued, never carried: it waits on the next pass, which it names.
    let (listed, stdout, stderr) = journey.run(&["change", "list", &checkout]);
    assert!(listed, "list: {stderr}");
    assert!(
        stdout.contains(&format!(
            "waits on: the next pass of the queue; unblock: `stado change run {checkout}`, or \
             the resident pass"
        )),
        "a queued candidate names the pass it waits on: {stdout}"
    );

    // Main moves past the candidate's base, so the candidate cannot merge.
    journey.git(&["checkout", "--quiet", "--detach", &base]);
    let moved = commit(&mut journey, "notes.txt", "moved\n", "main moves");
    journey.git(&[
        "push",
        "--quiet",
        "origin",
        &format!("{moved}:refs/heads/main"),
    ]);
    journey.git(&["checkout", "--quiet", "main"]);

    // The remote refuses: the candidate stays queued and says it waits on it.
    let ssh_origin = format!("ssh://fixture.invalid/{OWNER}/{NAME}.git");
    journey.origin(&ssh_origin, Some("false"));
    let (ran, stdout, stderr) = journey.run(&["change", "run", &checkout]);
    assert!(
        !ran,
        "a pass whose remote refuses exits non-zero: {stdout}{stderr}"
    );
    let (_, stdout, _) = journey.run(&["change", "list", &checkout]);
    assert!(
        stdout.contains(&format!(
            "waits on: the remote; origin of {checkout} did not answer a fetch or a push"
        )),
        "a candidate whose pass stopped at the remote names it: {stdout}"
    );
    assert_eq!(journey.candidate(&id)["waits_on"], json!("remote"));

    // The remote hangs; the pass is killed while it waits, as the end of a
    // session or a restart of its machine kills it. Git appends the host and
    // the remote command to `core.sshCommand`, which `; :` swallows.
    journey.origin(&ssh_origin, Some("sleep 120; :"));
    let mut pass = journey
        .stado()
        .args(["change", "run", &checkout])
        .process_group(0)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let group = nix::unistd::Pid::from_raw(i32::try_from(pass.id()).unwrap());
    let started = Instant::now();
    loop {
        if journey.candidate(&id)["state"] == json!("running") {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "the pass never took the candidate: {}",
            journey.candidate(&id)
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let killed = journey.candidate(&id)["pass"]["pid"].clone();
    assert_eq!(
        killed,
        json!(pass.id()),
        "the running candidate names its pass"
    );
    nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL).unwrap();
    pass.wait().unwrap();
    journey.report["killed_pass"] = killed.clone();

    let (_, stdout, _) = journey.run(&["change", "list", &checkout]);
    assert!(
        stdout.contains(&format!(
            "waits on: a pass; the pass that carried it (pid {killed}) is gone (killed, or its \
             machine restarted) and holds no lock of {OWNER}/{NAME}; unblock: `stado change run \
             {checkout}`, which resumes it, or the resident pass of the queue"
        )),
        "a candidate left running by a killed pass is named as such: {stdout}"
    );
    let (_, stdout, _) = journey.run(&["change", "status", &id, "--json"]);
    let status: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(status["state"], json!("running"));
    assert!(
        status["waits"]
            .as_str()
            .unwrap()
            .starts_with("waits on: a pass;"),
        "status says what it waits on: {status}"
    );

    // The resident pass names no checkout: it finds the repository from the
    // candidate, resumes it and carries it to a verdict.
    journey.origin(&remote, None);
    let (ran, stdout, stderr) = journey.run(&["change", "run", "--json"]);
    assert!(ran, "the resumed pass reaches a verdict: {stdout}{stderr}");
    assert!(
        stderr.contains(&format!(
            "{id} was left running by a pass that is gone; this pass resumes it"
        )),
        "the pass says it resumes the candidate: {stderr}"
    );
    let after = journey.candidate(&id);
    assert_eq!(after["state"], json!("refused"), "{after}");
    assert!(
        after["refusal"]
            .as_str()
            .unwrap()
            .contains("does not merge onto main"),
        "{after}"
    );
    assert_eq!(after["pass"], Value::Null);
    let (_, stdout, _) = journey.run(&["change", "list", &checkout]);
    assert!(
        !stdout.contains("waits on:"),
        "a candidate at a verdict waits on nothing: {stdout}"
    );
    let (ran, stdout, _) = journey.run(&["change", "run"]);
    assert!(ran);
    assert!(
        stdout.contains("no candidate of any repository of this machine's queue waits for a pass"),
        "{stdout}"
    );

    journey.report["outcome"] = json!("passed");
    journey.save();
}

/// A high-risk candidate on a machine without Tama waits for a review round
/// recorded with `stado change review` by the session the command runs in.
/// A command no session runs, the author's own session and a findings file
/// that is no JSON array are refused; a round over a tree a moved `main`
/// replaced decides nothing; one reviewer's later empty round does not
/// clear another's findings; and once every reviewer's newest round over
/// the tree the pass lands found nothing, it lands on the bare `origin`.
#[test]
fn a_round_recorded_with_change_review_decides_the_review_before_landing() {
    let mut journey = Journey::start();
    // A format gate the export runs, and no compiling gate, which makes
    // every change of the product high risk: reviewed before it lands.
    let base = repository(
        &mut journey,
        json!([{"name": "style", "kind": "format", "argv": ["true"]}]),
    );
    let checkout = journey.checkout.to_string_lossy().into_owned();
    let reviewed = commit(&mut journey, "notes.txt", "two\n", "a reviewed change");
    let (submitted, stdout, stderr) = journey.run_in_session(
        "author-session",
        &[
            "change", "submit", &checkout, "--commit", &reviewed, "--defect", DEFECT, "--json",
        ],
    );
    assert!(submitted, "submit: {stdout}{stderr}");
    let id = serde_json::from_str::<Value>(&stdout).unwrap()["candidates"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(journey.candidate(&id)["session"], json!("author-session"));
    let none = journey.root.join("none.json");
    fs::write(&none, "[]").unwrap();
    let found = journey.root.join("found.json");
    fs::write(
        &found,
        r#"["notes.txt says two where the change means three"]"#,
    )
    .unwrap();
    let none_path = none.to_string_lossy().into_owned();
    let found_path = found.to_string_lossy().into_owned();
    let review = |findings: &str| -> Vec<String> {
        let argv: [&str; 7] = [
            "change",
            "review",
            id.as_str(),
            "--findings",
            findings,
            "--improvements",
            none_path.as_str(),
        ];
        argv.map(str::to_string).to_vec()
    };
    fn words(args: &[String]) -> Vec<&str> {
        args.iter().map(String::as_str).collect()
    }

    // An early round over the candidate's own commit, before main moves.
    let (recorded, stdout, stderr) =
        journey.run_in_session("reviewer-early", &words(&review(&none_path)));
    assert!(recorded, "{stdout}{stderr}");
    // Main moves: the pass writes the candidate again on the new tip, a
    // tree the early reviewer never read.
    journey.git(&["checkout", "--quiet", "--detach", &base]);
    let moved = commit(&mut journey, "other.txt", "moved\n", "main moves");
    journey.git(&[
        "push",
        "--quiet",
        "origin",
        &format!("{moved}:refs/heads/main"),
    ]);
    journey.git(&["checkout", "--quiet", "main"]);

    // No round over the rebased tree and no Tama: the pass stops at review.
    let (ran, stdout, stderr) = journey.run(&["change", "run", &checkout]);
    assert!(
        !ran,
        "a pass with no round over its tree exits non-zero: {stdout}{stderr}"
    );
    let held = journey.candidate(&id);
    assert_eq!(held["state"], json!("queued"), "{held}");
    assert_eq!(held["waits_on"], json!("review-round"), "{held}");
    let rebased = held["rebased"]["commit"].as_str().unwrap().to_string();
    let tree = held["rebased"]["tree"].as_str().unwrap().to_string();
    assert_ne!(
        rebased, reviewed,
        "the pass wrote the candidate again on the moved main"
    );
    assert!(
        held["refusal"].as_str().unwrap().contains(&format!(
            "no review round over commit {rebased} (tree {tree}) of candidate {id} is recorded \
             by a session that is not its author (author-session), and Tama, whose harsh review \
             would run one, is not installed here"
        )),
        "{held}"
    );
    let (_, stdout, _) = journey.run(&["change", "list", &checkout]);
    assert!(
        stdout.contains(&format!(
            "waits on: a review round over commit {rebased} by a session that is not its author \
             (author-session); unblock: a reviewer reads `git -C {checkout} show {rebased}` and, \
             from its own session's shell, records `stado change review {id} --findings FILE \
             --improvements FILE`"
        )),
        "{stdout}"
    );

    // A command no session runs records nothing.
    let (recorded, _, stderr) = journey.run(&words(&review(&none_path)));
    assert!(!recorded);
    assert!(
        stderr.contains("no agent session runs this command"),
        "{stderr}"
    );
    // The author cannot review its own candidate.
    let (recorded, _, stderr) =
        journey.run_in_session("author-session", &words(&review(&none_path)));
    assert!(!recorded);
    assert!(
        stderr.contains(&format!(
            "session author-session submitted candidate {id}, so it cannot review it: a round \
             counts only from a reviewer that is not the author"
        )),
        "{stderr}"
    );
    // A findings file that is no JSON array is refused naming the file.
    let (recorded, _, stderr) = journey.run_in_session("reviewer-one", &words(&review(&checkout)));
    assert!(!recorded);
    assert!(
        stderr.contains(&format!("--findings {checkout} cannot be read")),
        "{stderr}"
    );

    // One reviewer finds something; a second's later empty round does not
    // clear it.
    let (recorded, stdout, stderr) =
        journey.run_in_session("reviewer-one", &words(&review(&found_path)));
    assert!(recorded, "{stderr}");
    assert!(
        stdout.contains(&format!("over commit {rebased} (tree {tree})")),
        "the round is over the tree the pass lands: {stdout}"
    );
    let (recorded, _, stderr) = journey.run_in_session("reviewer-two", &words(&review(&none_path)));
    assert!(recorded, "{stderr}");
    let (ran, stdout, stderr) = journey.run(&["change", "run", &checkout]);
    assert!(ran, "a refusal is a verdict: {stdout}{stderr}");
    let refused = journey.candidate(&id);
    assert_eq!(refused["state"], json!("refused"), "{refused}");
    assert!(
        refused["refusal"]
            .as_str()
            .unwrap()
            .contains("session reviewer-one at"),
        "{refused}"
    );
    assert!(
        refused["refusal"]
            .as_str()
            .unwrap()
            .contains("notes.txt says two where the change means three"),
        "{refused}"
    );

    // The repaired change: every reviewer's newest round found nothing.
    let repaired = commit(&mut journey, "notes.txt", "three\n", "the repaired change");
    let (submitted, stdout, stderr) = journey.run_in_session(
        "author-session",
        &[
            "change", "submit", &checkout, "--commit", &repaired, "--defect", DEFECT, "--json",
        ],
    );
    assert!(submitted, "submit: {stdout}{stderr}");
    let id = serde_json::from_str::<Value>(&stdout).unwrap()["candidates"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (ran, _, _) = journey.run(&["change", "run", &checkout]);
    assert!(!ran, "no round over the repaired tree yet");
    for reviewer in ["reviewer-one", "reviewer-two"] {
        let (recorded, _, stderr) = journey.run_in_session(
            reviewer,
            &[
                "change",
                "review",
                &id,
                "--findings",
                &none_path,
                "--improvements",
                &none_path,
            ],
        );
        assert!(recorded, "{stderr}");
    }
    let (ran, stdout, stderr) = journey.run(&["change", "run"]);
    assert!(ran, "the reviewed candidate lands: {stdout}{stderr}");
    let landed = journey.candidate(&id);
    assert_eq!(landed["state"], json!("landed"), "{landed}");
    let review = landed["reviews"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(review["verdict"], json!("admitted"), "{landed}");
    let remote = journey.remote.to_string_lossy().into_owned();
    let main = journey.git(&["--git-dir", &remote, "rev-parse", "refs/heads/main"]);
    assert_eq!(landed["landed"]["commit"], json!(main), "{landed}");
    let shown = journey.git(&["--git-dir", &remote, "show", "refs/heads/main:notes.txt"]);
    assert_eq!(shown, "three", "origin's main holds the reviewed change");
    journey.report["landed"] = json!({"candidate": id, "commit": repaired, "main": main});
    journey.report["outcome"] = json!("passed");
    journey.save();
}

/// The size cap measures a candidate's own commits: past its merge base
/// with `main`, or, stacked on a candidate still waiting, past that
/// candidate. The product's catalog lets one change change 400 lines.
#[test]
fn a_stacked_candidate_is_measured_over_the_candidate_below_it() {
    let mut journey = Journey::start();
    let base = repository(&mut journey, json!([]));
    let checkout = journey.checkout.to_string_lossy().into_owned();
    let lines = |count: usize, tag: &str| -> String {
        (0..count).map(|line| format!("{tag} {line}\n")).collect()
    };
    let submit = |journey: &mut Journey, commit: &str| {
        journey.run(&[
            "change", "submit", &checkout, "--commit", commit, "--defect", DEFECT, "--json",
        ])
    };

    // 300 lines over main: within the cap, queued.
    let below = commit(
        &mut journey,
        "below.txt",
        &lines(300, "below"),
        "the change below",
    );
    let (submitted, stdout, stderr) = submit(&mut journey, &below);
    assert!(submitted, "submit: {stdout}{stderr}");

    // 150 more on top: 450 over main, 150 over the candidate below it.
    let above = commit(
        &mut journey,
        "above.txt",
        &lines(150, "above"),
        "the change above",
    );
    let (submitted, stdout, stderr) = submit(&mut journey, &above);
    assert!(
        submitted,
        "a stacked candidate is measured over the one below it: {stdout}{stderr}"
    );
    let above_id = serde_json::from_str::<Value>(&stdout).unwrap()["candidates"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // The same 450 lines in one commit straight on main are refused.
    journey.git(&["checkout", "--quiet", "--detach", &base]);
    fs::write(journey.checkout.join("above.txt"), lines(150, "above")).unwrap();
    journey.git(&["add", "above.txt"]);
    let whole = commit(
        &mut journey,
        "below.txt",
        &lines(300, "below"),
        "one big change",
    );
    let (submitted, _, stderr) = submit(&mut journey, &whole);
    assert!(!submitted, "450 lines over main exceed the cap");
    assert!(
        stderr.contains(&format!(
            "{whole} changes 450 lines in 2 files past its merge base with main ({base}), more \
             than the 400 one change of {PRODUCT} may"
        )),
        "{stderr}"
    );

    // Stacked on a waiting candidate, the stacked part alone counts: 450
    // lines over the nearest candidate below are refused naming it.
    journey.git(&["checkout", "--quiet", "--detach", &above]);
    let big = commit(
        &mut journey,
        "big.txt",
        &lines(450, "big"),
        "a big stacked change",
    );
    let (submitted, _, stderr) = submit(&mut journey, &big);
    assert!(!submitted);
    assert!(
        stderr.contains(&format!(
            "{big} changes 450 lines in 1 files past candidate {above_id} it is stacked on \
             ({above})"
        )),
        "the nearest waiting candidate is the base: {stderr}"
    );
    journey.report["outcome"] = json!("passed");
    journey.save();
}
