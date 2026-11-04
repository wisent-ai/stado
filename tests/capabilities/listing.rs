//! `stado capabilities list`, run as the real binary in an isolated home:
//! the listing primitive answers its own `list` verb with the catalog the
//! bare command prints, whole and for one family, in text and as JSON; a
//! family it does not declare is refused naming every family, under the verb
//! as under the bare form, and writes nothing.
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

struct Home {
    root: PathBuf,
}

impl Home {
    fn new() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the package sits inside the checkout")
            .to_path_buf();
        let root = repository
            .join(".build/capabilities-listing")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("store")).unwrap();
        let home = Self { root };
        let initialised = home.run(&["config", "init"]);
        assert!(initialised.status.success(), "{}", said(&initialised));
        home
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH names the tools"))
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn printed(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(output.status.success(), "stado {args:?}: {}", said(&output));
        String::from_utf8(output.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.printed(args)).unwrap()
    }

    /// What the listing reads and a capability write would change: the
    /// record of proofs and service (`~/.stado/capabilities.json`) and the
    /// configuration holding each family's order. Every run also notes its
    /// caller under `~/.stado/callers` (`stado_product::callers`), which is
    /// no capability state.
    fn capability_state(&self) -> Vec<Option<Vec<u8>>> {
        let stado = self.root.join(".stado");
        ["capabilities.json", "config.json"]
            .iter()
            .map(|name| std::fs::read(stado.join(name)).ok())
            .collect()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn list_prints_the_catalog_the_bare_command_prints() {
    let home = Home::new();

    let bare = home.json(&["capabilities", "--json"]);
    let listed = home.json(&["capabilities", "list", "--json"]);
    assert_eq!(listed, bare, "list --json is the bare catalog");
    let families = listed["capabilities"].as_array().expect("the catalog lists capabilities");
    assert!(families.len() > 1, "the whole catalog holds every family: {listed}");

    let family = families[0]["id"].as_str().expect("a capability names its id");
    let one = home.json(&["capabilities", "list", family, "--json"]);
    assert_eq!(one, home.json(&["capabilities", family, "--json"]));
    let ids: Vec<&str> = one["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|capability| capability["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![family], "list FAMILY restricts the catalog to that family");

    assert_eq!(
        home.printed(&["capabilities", "list"]),
        home.printed(&["capabilities"]),
        "the text listing is the bare one"
    );
}

#[test]
fn an_undeclared_family_is_refused_naming_every_family_and_writes_nothing() {
    let home = Home::new();
    let family = home.json(&["capabilities", "--json"])["capabilities"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let before = home.capability_state();

    for args in [
        &["capabilities", "list", "no-such-family"][..],
        &["capabilities", "no-such-family"][..],
    ] {
        let refused = home.run(args);
        assert!(!refused.status.success(), "stado {args:?} was not refused");
        let text = said(&refused);
        assert!(
            text.contains("unknown capability \"no-such-family\"; use one of:") && text.contains(&family),
            "stado {args:?} does not name the families: {text}"
        );
    }
    assert_eq!(home.capability_state(), before, "a refused listing changed the capability state");
}
