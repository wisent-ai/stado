//! The real `wisent-products` executable, resolved the way `stado product`
//! resolves it, and one isolated home both of them run under.
//!
//! Isolation is of DATA, never the component: a temp `HOME` so the operator's
//! `~/.stado/products` records cannot decide what a refusal says, a temp
//! store, and a `STADO_CONFIG` that does not exist. The executable found on
//! `PATH` is the operator's own installed one.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Output};

/// Where the real installer may be found, in one fixed order:
/// `WISENT_PRODUCTS_BIN`, then `PATH`, then `~/.stado/bin`. When none of the
/// three holds it, this panics with one sentence naming what is missing: a
/// test that cannot find the real thing must say so, not measure a substitute.
pub fn wisent_products() -> PathBuf {
    if let Some(declared) = std::env::var_os("WISENT_PRODUCTS_BIN") {
        let path = PathBuf::from(declared);
        assert!(
            path.is_file(),
            "WISENT_PRODUCTS_BIN names {} which is not a file; point it at the real \
             wisent-products executable",
            path.display()
        );
        return path;
    }
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .map(|directory| directory.join("wisent-products"))
        .find(|candidate| candidate.is_file());
    if let Some(found) = on_path {
        return found;
    }
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
    let in_stado_bin = home.join(".stado/bin/wisent-products");
    assert!(
        in_stado_bin.is_file(),
        "the real wisent-products executable is missing: set WISENT_PRODUCTS_BIN, or put \
         wisent-products on PATH, or install it at ~/.stado/bin/wisent-products with \
         `pipx install git+https://github.com/wisent-ai/wisent-products`"
    );
    in_stado_bin
}

/// One isolated home and store, and the real installer reachable exactly the
/// way `stado product` looks for it.
pub struct Delegation {
    home: tempfile::TempDir,
    storage: PathBuf,
    installer: PathBuf,
    path: OsString,
}

impl Delegation {
    pub fn new() -> Self {
        let installer = wisent_products();
        assert_eq!(
            installer.file_name().and_then(|name| name.to_str()),
            Some("wisent-products"),
            "the resolved installer {} is not named wisent-products, so `stado product` cannot \
             find it on PATH; point WISENT_PRODUCTS_BIN at the executable itself",
            installer.display()
        );
        let directory = installer
            .parent()
            .expect("the installer has a parent directory")
            .to_path_buf();
        let home = tempfile::Builder::new()
            .prefix("product-")
            .tempdir()
            .unwrap();
        let storage = home.path().join("store");
        std::fs::create_dir_all(&storage).unwrap();
        // The installer's own directory first, then the system directories its
        // interpreter needs. Nothing this test wrote is on this PATH.
        let mut path = OsString::from(directory);
        path.push(":/usr/bin:/bin");
        Self {
            home,
            storage,
            installer,
            path,
        }
    }

    /// The real installer, invoked directly, under the same isolated home.
    pub fn installer(&self, args: &[&str]) -> Output {
        Command::new(&self.installer)
            .args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", &self.path)
            .output()
            .expect("the real wisent-products executable runs")
    }

    /// The built product binary, which must reach the same installer.
    pub fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", &self.path)
            .env("NO_COLOR", "1")
            .env("STADO_CONFIG", self.home.path().join("no-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local")
            .output()
            .expect("stado runs")
    }

    /// Product surfaces the installer has recorded as installed under this
    /// home. It keeps them in `~/.stado/products/<id>`; an empty list is the
    /// durable proof that a refusal installed nothing.
    pub fn recorded_products(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.home.path().join(".stado/products"))
            .into_iter()
            .flatten()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
