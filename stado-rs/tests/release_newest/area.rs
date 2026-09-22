//! The isolated workspace these cases run against.
//!
//! A real object store under the test's own directory, real Git checkouts
//! with real commits, and every command's arguments, output and exit status
//! written beside them. The operator's own workspace is never read: the
//! command is always given a `--root` inside this area.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The manifest schema the release pipeline declares as `SCHEMA_VERSION`; a
/// fixture writing any other number is refused by the parser under test.
const MANIFEST_SCHEMA: u32 = 1;

pub struct Area {
    pub root: PathBuf,
    pub workspace: PathBuf,
    storage: PathBuf,
    commands: std::cell::Cell<usize>,
}

impl Area {
    pub fn new(name: &str) -> Self {
        let evidence = Path::new(env!("CARGO_MANIFEST_DIR")).join(".wisent-output/release-newest");
        std::fs::create_dir_all(&evidence).expect("create retained evidence directory");
        let root = tempfile::Builder::new()
            .prefix(&format!("{name}-"))
            .tempdir_in(evidence)
            .expect("create the isolated area")
            .keep();
        let workspace = root.join("workspace");
        let storage = root.join("storage");
        std::fs::create_dir_all(&workspace).expect("create the workspace");
        std::fs::create_dir_all(&storage).expect("create the isolated object store");
        let area = Self {
            root,
            workspace,
            storage,
            commands: std::cell::Cell::new(0),
        };
        eprintln!("release newest evidence: {}", area.root.display());
        area
    }

    pub fn stado(&self, args: &[&str]) -> Output {
        let index = self.commands.get();
        self.commands.set(index + 1);
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            // A set-but-missing config disables configuration discovery, so
            // the operator's own deployment is never addressed.
            .env("STADO_CONFIG", self.root.join("no-such-config.json"))
            .env("NO_COLOR", "1")
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("STADO_API_URL")
            .env_remove("WC_PROFILES_DIR")
            .output()
            .expect("the stado binary runs");
        let directory = self.root.join("commands").join(index.to_string());
        std::fs::create_dir_all(&directory).expect("create command evidence directory");
        std::fs::write(directory.join("args"), args.join(" ")).expect("record the command");
        std::fs::write(directory.join("stdout"), &output.stdout).expect("record stdout");
        std::fs::write(directory.join("stderr"), &output.stderr).expect("record stderr");
        std::fs::write(
            directory.join("exit"),
            output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_default(),
        )
        .expect("record the exit status");
        output
    }

    /// `stado release newest --plan --json` over this area's workspace.
    pub fn plan(&self, extra: &[&str]) -> Output {
        let workspace = self.workspace.display().to_string();
        let mut args = vec![
            "release",
            "newest",
            "--root",
            workspace.as_str(),
            "--plan",
            "--json",
        ];
        args.extend_from_slice(extra);
        self.stado(&args)
    }

    /// A real checkout of one product: a manifest, a version file, and a
    /// commit that carries both.
    pub fn checkout(
        &self,
        product: &str,
        manifest: &str,
        version_file: Option<(&str, &str)>,
    ) -> PathBuf {
        let checkout = self.workspace.join(product);
        std::fs::create_dir_all(&checkout).expect("create the checkout");
        git(&checkout, &["init", "--initial-branch=main"]);
        git(&checkout, &["config", "user.email", "area@wisent.test"]);
        git(&checkout, &["config", "user.name", "release newest area"]);
        std::fs::write(checkout.join(".wisent-release.json"), manifest).expect("write manifest");
        if let Some((path, body)) = version_file {
            std::fs::write(checkout.join(path), body).expect("write the version file");
        }
        git(&checkout, &["add", "-A"]);
        git(&checkout, &["commit", "-m", "the product as it stands"]);
        checkout
    }
}

fn git(checkout: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(checkout)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A product that releases, numbered from its own `package.json`.
pub fn releasing_manifest(product: &str) -> String {
    serde_json::json!({
        "schema_version": MANIFEST_SCHEMA,
        "product": product,
        "releases": true,
        "version_source": { "kind": "json", "path": "package.json", "pointer": "/version" },
        "platforms": {
            "darwin-arm64": {
                "runner_platform": "darwin-arm64",
                "quality": [],
                "build": { "argv": ["true"] },
                "stage": { "bin/thing": "bin/thing" }
            }
        },
        "promotion": { "channels": ["candidate"], "reconcile": false }
    })
    .to_string()
}

/// A product that declares it ships nothing, and says why.
pub fn silent_manifest(product: &str, reason: &str) -> String {
    serde_json::json!({
        "schema_version": MANIFEST_SCHEMA,
        "product": product,
        "releases": false,
        "reason": reason,
    })
    .to_string()
}
