//! `stado storage archive`: one tree gives one archive, whoever made it.
//!
//! Two copies of the same tree differ in everything a builder's machine
//! leaves on files besides their content — modification times and the
//! group/other permission bits a umask decides. Expected: the two archives
//! have the same SHA-256. The output is named by a bare file name relative to
//! the working directory (its parent is the empty path), and an output inside
//! the source tree is refused naming why.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Deployment {
    root: PathBuf,
    report: Value,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the crate sits inside the repository")
            .to_path_buf();
        let root = repository
            .join(".build/storage-archive-reproducible")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        Self {
            root,
            report: json!({"commands": []}),
        }
    }

    fn run(&mut self, program: &str, args: &[&str]) -> Output {
        let output = Command::new(program)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .args(args)
            .output()
            .unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "program": program,
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
        output
    }

    fn succeed(&mut self, program: &str, args: &[&str]) -> String {
        let output = self.run(program, args);
        assert!(
            output.status.success(),
            "{program} {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// One copy of the tree under `name`, its files touched at `stamp` and
    /// their group/other bits set by `bits`.
    fn tree(&mut self, name: &str, stamp: &str, bits: &str) -> PathBuf {
        let tree = self.root.join(name);
        fs::create_dir_all(tree.join("bin")).unwrap();
        fs::create_dir_all(tree.join("share/doc")).unwrap();
        fs::write(tree.join("bin/tool"), "#!/bin/sh\necho tool\n").unwrap();
        fs::write(
            tree.join("share/doc/readme.txt"),
            "the same words in both copies\n",
        )
        .unwrap();
        let path = |relative: &str| tree.join(relative).to_string_lossy().into_owned();
        self.succeed("chmod", &["u+x", &path("bin/tool")]);
        for file in ["bin/tool", "share/doc/readme.txt"] {
            self.succeed("chmod", &[bits, &path(file)]);
            self.succeed("touch", &["-t", stamp, &path(file)]);
        }
        tree
    }

    fn archive(&mut self, source: &Path, output: &str) -> Value {
        let stado = env!("CARGO_BIN_EXE_stado");
        let printed = self.succeed(
            stado,
            &[
                "storage",
                "archive",
                &source.to_string_lossy(),
                output,
                "--json",
            ],
        );
        serde_json::from_str(&printed).unwrap()
    }
}

#[test]
fn one_tree_archives_to_one_digest_whatever_its_times_and_umask() {
    let mut deployment = Deployment::start();
    let stado = env!("CARGO_BIN_EXE_stado");
    deployment.succeed(stado, &["config", "init"]);
    let first = deployment.tree("first", "200001010000", "go+r");
    let second = deployment.tree("second", "202406151230", "go-rwx");
    let one = deployment.archive(&first, "one.tar.gz");
    let two = deployment.archive(&second, "two.tar.gz");
    assert!(
        deployment.root.join("one.tar.gz").is_file(),
        "a bare output name lands in the working directory: {one}"
    );
    assert_eq!(
        one["sha256"], two["sha256"],
        "the same tree gave two archives: {one} and {two}"
    );

    let inside = first.join("inside.tar.gz");
    let refused = deployment.run(
        stado,
        &[
            "storage",
            "archive",
            &first.to_string_lossy(),
            &inside.to_string_lossy(),
        ],
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success(),
        "an output inside the source must be refused: {stderr}"
    );
    assert!(
        stderr.contains("archive output must be outside the source directory"),
        "the refusal names why: {stderr}"
    );
    assert!(
        !inside.exists(),
        "a refused archive writes nothing: {}",
        inside.display()
    );
}
