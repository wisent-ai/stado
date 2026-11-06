//! A checkout whose recorded `origin/HEAD` names a branch the origin has
//! since deleted is still read on its origin's default branch.
//!
//! A clone records the default branch it saw (`refs/remotes/origin/HEAD`).
//! When that branch is deleted on the origin and pruned from the checkout,
//! the record stays behind as a dangling symbolic ref, and Stado read it as
//! the branch the checkout must work on: a checkout on `main`, the origin's
//! default now, was refused as "not on <deleted branch>", and `stado quality
//! hooks status` could not take it, so its push gate was never installed.
//!
//! A real workspace under this checkout's `.build` with no catalog file, so
//! the real `stado` reads the catalog compiled into it, and an isolated
//! `HOME` whose Git config sends the GitHub address of that catalog's
//! `trends` repository to a bare repository of the journey's own
//! (`url.<bare>.insteadOf`), so the checkout's recorded origin is the one
//! Stado identifies it by and no network is reached. The checkout is cloned
//! while the bare repository's default is `old`; `old` is then renamed to
//! `main` there and made the default, and the checkout fetched with
//! `--prune` and switched to `main`, so its `origin/HEAD` dangles. `stado
//! quality hooks status --product trends` reads the checkout and answers its
//! push gate (`absent`), not `unreadable`. With the record pointing at the
//! existing `origin/main`, a checkout left on a feature branch is still read
//! as off its canonical branch.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// A product of the compiled catalog whose every repository is one.
const PRODUCT: &str = "trends";
const REPOSITORY: &str = "wisent-ai/trends";

struct Workspace {
    root: PathBuf,
    home: PathBuf,
    workspace: PathBuf,
    bare: PathBuf,
    checkout: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.build/journeys/quality-hooks-dangling-head")
            .join(uuid::Uuid::new_v4().simple().to_string());
        let home = root.join("home");
        let workspace = root.join("workspace");
        let bare = root.join("remote.git");
        let checkout = workspace.join("trends");
        for directory in [&home, &workspace] {
            std::fs::create_dir_all(directory).expect("create the journey's directories");
        }
        let place = Self { root, home, workspace, bare, checkout };
        let bare_url = format!("file://{}", place.bare.display());
        let github = format!("https://github.com/{REPOSITORY}");
        place.git(&place.home, &["config", "--global", &format!("url.{bare_url}.insteadOf"), &github]);
        place.git(&place.home, &["config", "--global", "user.name", "Hooks Journey"]);
        place.git(&place.home, &["config", "--global", "user.email", "hooks-journey@example.invalid"]);
        place
    }

    fn git(&self, at: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(at)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .args(args)
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn hooks_status(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("WISENT_WORKSPACE", &self.workspace)
            .env(
                "STADO_CONFIG",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/config/empty.json"),
            )
            .args(["quality", "hooks", "status", "--product", PRODUCT, "--json"])
            .output()
            .expect("run the real stado")
    }

    /// The row `hooks status` answers for the fixture's repository.
    fn row(&self) -> Value {
        let output = self.hooks_status();
        let answer: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "hooks status printed no JSON ({error}): {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        answer["checkouts"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["repository"] == REPOSITORY)
            .cloned()
            .unwrap_or_else(|| panic!("hooks status names no row for {REPOSITORY}: {answer}"))
    }
}

#[test]
fn a_checkout_whose_origin_head_dangles_is_read_on_the_origin_default() {
    let place = Workspace::new();
    // The bare repository starts with `old` as its default branch.
    let seed = place.root.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    place.git(&seed, &["init", "--quiet", "--initial-branch", "old"]);
    std::fs::write(seed.join("README.md"), "fixture\n").unwrap();
    place.git(&seed, &["add", "README.md"]);
    place.git(&seed, &["commit", "--quiet", "-m", "The fixture"]);
    let bare = place.bare.display().to_string();
    place.git(&place.root, &["clone", "--quiet", "--bare", &seed.display().to_string(), &bare]);

    // Cloned while `old` is the default: origin/HEAD records origin/old.
    let github = format!("https://github.com/{REPOSITORY}");
    place.git(&place.workspace, &["clone", "--quiet", &github, &place.checkout.display().to_string()]);
    assert_eq!(place.git(&place.checkout, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]), "origin/old");

    // The origin renames `old` to `main` and makes it the default.
    place.git(&place.bare, &["branch", "-m", "old", "main"]);
    place.git(&place.bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    place.git(&place.checkout, &["fetch", "--quiet", "--prune", "origin"]);
    place.git(&place.checkout, &["switch", "--quiet", "--create", "main", "--track", "origin/main"]);
    place.git(&place.checkout, &["branch", "--quiet", "-D", "old"]);
    let pruned = Command::new("git")
        .current_dir(&place.checkout)
        .env("HOME", &place.home)
        .args(["rev-parse", "--verify", "--quiet", "refs/remotes/origin/old"])
        .output()
        .unwrap();
    assert!(!pruned.status.success(), "the fixture's origin/old was not pruned");

    let row = place.row();
    assert_ne!(row["state"], "unreadable", "a checkout on the origin's default was not read: {row}");
    assert_eq!(row["state"], "absent", "{row}");

    // With the record naming an existing branch, a checkout off it is still
    // refused as off its canonical branch: the command fails, printing no
    // document, and its refusal names the branch.
    place.git(&place.checkout, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main"]);
    place.git(&place.checkout, &["switch", "--quiet", "--create", "feature"]);
    let off = place.hooks_status();
    let said = format!("{}{}", String::from_utf8_lossy(&off.stdout), String::from_utf8_lossy(&off.stderr));
    assert!(!off.status.success(), "a checkout off its canonical branch was taken: {said}");
    assert!(said.contains("is not on main"), "the refusal does not name the canonical branch: {said}");
}
