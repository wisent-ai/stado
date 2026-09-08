//! `stado product` against the real `wisent-products` executable.
//!
//! Nothing is stubbed. `stado product` owns no catalogue and no installer of
//! its own — `cli::product` resolves `wisent-products` and hands it the verb —
//! so the only evidence worth having is a comparison against what that
//! executable produces on its own. Every assertion below therefore runs the
//! real binary directly and then through Stado, and compares.
//!
//! What this replaced, and why: the two tests here used to write a shell
//! script named `wisent-products` onto `PATH` that echoed its own argv back as
//! JSON, and asserted the echo. An argv echo is a thing only a stub can
//! produce, so the assertion could never fail for the reason it claimed to
//! defend — a Stado that forwarded nothing but spawned the script correctly
//! passed it. The refusals below come from the real installer's own mouth and
//! name the argument that produced them, which is the same proof, for real.
//!
//! Isolation is of DATA, never the component: a temp `HOME` so the operator's
//! `~/.stado/products` records cannot decide what a refusal says, a temp
//! store, and a `STADO_CONFIG` that does not exist. The executable found on
//! `PATH` is the operator's own installed one.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

/// Where the real installer may be found, in one fixed order:
/// `WISENT_PRODUCTS_BIN`, then `PATH`, then `~/.stado/bin`. When none of the
/// three holds it, this panics with one sentence naming what is missing: a
/// test that cannot find the real thing must say so, not measure a substitute.
fn wisent_products() -> PathBuf {
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
struct Delegation {
    home: tempfile::TempDir,
    storage: PathBuf,
    installer: PathBuf,
    path: OsString,
}

impl Delegation {
    fn new() -> Self {
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
    fn installer(&self, args: &[&str]) -> Output {
        Command::new(&self.installer)
            .args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", &self.path)
            .output()
            .expect("the real wisent-products executable runs")
    }

    /// The built product binary, which must reach the same installer.
    fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", &self.path)
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
    fn recorded_products(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.home.path().join(".stado/products"))
            .into_iter()
            .flatten()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// `stado product catalog` must hand back the external catalogue, unaltered.
///
/// Stado also compiles a service catalogue of its own
/// (`deploy::service_catalog`), generated from the same upstream file, and a
/// command that answered out of it would look right and then drift unnoticed.
/// The assertion is therefore byte equality with what the external executable
/// prints for itself: the only way to reproduce 30 kB of another program's
/// output exactly is to have asked it.
#[test]
fn product_catalog_is_the_real_external_catalog() {
    let journey = Delegation::new();

    let direct = journey.installer(&["catalog", "--json"]);
    assert!(
        direct.status.success(),
        "the real wisent-products refused to print its catalogue: {}",
        stderr(&direct)
    );
    let through_stado = journey.stado(&["product", "catalog", "--json"]);
    assert!(
        through_stado.status.success(),
        "stado product catalog failed: {}",
        stderr(&through_stado)
    );

    assert_eq!(
        String::from_utf8_lossy(&through_stado.stdout),
        String::from_utf8_lossy(&direct.stdout),
        "stado product catalog did not hand back the external catalogue verbatim, so it is \
         answering out of something other than wisent-products"
    );

    let catalog: Value = serde_json::from_slice(&through_stado.stdout)
        .expect("the external catalogue is one JSON document");
    let products = catalog["products"]
        .as_array()
        .expect("the catalogue is a list of products");
    assert_eq!(
        products[0]["id"], "ster",
        "the canonical catalogue opens with ster: {catalog:#}"
    );
    // Every product the rest of Stado names by hand must be in the catalogue
    // it delegates to, or `product install` cannot install what it deploys.
    for expected in ["ster", "stado", "brama", "skarbiec", "weles", "probierz"] {
        assert!(
            products.iter().any(|product| product["id"] == expected),
            "the external catalogue does not hold {expected}"
        );
    }
    for product in products {
        assert!(
            product["id"].as_str().is_some_and(|id| !id.is_empty()),
            "a catalogue record carries no id: {product:#}"
        );
        assert!(
            product["surfaces"]
                .as_array()
                .is_some_and(|surfaces| !surfaces.is_empty()),
            "catalogue record {} declares no installable surface",
            product["id"]
        );
    }

    // Reading the catalogue is delegation, not local bookkeeping: it must
    // leave no product record behind in the isolated home.
    assert!(
        journey.recorded_products().is_empty(),
        "reading the catalogue recorded installations: {:?}",
        journey.recorded_products()
    );
}

/// A refused install must reach the operator as the installer's own sentence.
///
/// The product and the surface are both proven, and proven separately: the
/// same product refused on two different surfaces produces two different
/// sentences from the real installer, so a Stado that dropped or rewrote
/// `--surface` cannot produce the matching pair. Neither sentence exists
/// anywhere in Stado's source — it can only be relaying them.
///
/// `las` is a catalogue product with a `cli` recipe and no service or desktop
/// one, so both refusals happen at recipe lookup: no checkout is touched, no
/// host is contacted, nothing is installed.
#[test]
fn product_install_surfaces_the_external_refusal_verbatim() {
    let journey = Delegation::new();

    for (surface, sentence) in [
        ("service", "las has no service installation recipe"),
        ("desktop", "las has no desktop installation recipe"),
    ] {
        let direct = journey.installer(&["install", "las", "--surface", surface, "--json"]);
        assert_eq!(
            stderr(&direct).trim(),
            format!("Error: {sentence}"),
            "the real installer's refusal for surface {surface} has changed; re-probe it before \
             asserting a sentence"
        );

        let through_stado = journey.stado(&[
            "product",
            "install",
            "las",
            "--surface",
            surface,
            "--host",
            "fleet-probe",
            "--json",
        ]);
        assert_eq!(
            through_stado.status.code(),
            direct.status.code(),
            "stado did not carry the installer's exit status for surface {surface}: {}",
            stderr(&through_stado)
        );
        assert!(
            stderr(&through_stado).contains(sentence),
            "stado did not surface the installer's own refusal {sentence:?} for surface \
             {surface}: {}",
            stderr(&through_stado)
        );
    }

    // And the product argument itself, forwarded verbatim: a name no
    // catalogue holds is refused by the installer in its own words, quoting
    // back the exact string Stado passed on.
    let unknown = journey.stado(&[
        "product",
        "install",
        "nieistnieje",
        "--surface",
        "cli",
        "--json",
    ]);
    assert!(
        !unknown.status.success(),
        "an unknown product was not refused: {}",
        String::from_utf8_lossy(&unknown.stdout)
    );
    assert!(
        stderr(&unknown).contains("unknown Wisent product 'nieistnieje'"),
        "stado did not surface the installer's unknown-product refusal: {}",
        stderr(&unknown)
    );

    // Refused means refused: the installer records every installation under
    // `~/.stado/products`, and after three refusals there is nothing there.
    assert!(
        journey.recorded_products().is_empty(),
        "a refused install left a product record behind: {:?}",
        journey.recorded_products()
    );
}
