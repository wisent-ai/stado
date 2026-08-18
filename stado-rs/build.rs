//! Embed the repository's machine-side enrollment bootstrap script.
//!
//! `GET /join.sh` on the dashboard must hand the joining machine exactly the
//! script that lives in the repository at `deploy/join.sh` — the script is not
//! written in Rust and is never templated. The served binary has to carry it,
//! because the dashboard runs from an installed binary with no repository
//! checkout beside it. A missing script is not a build failure: the route
//! answers 503 when the copy is empty, which is what happens in build contexts
//! whose source tree does not include `deploy/`.

use std::path::Path;

fn main() {
    let source = Path::new("..").join("deploy").join("join.sh");
    println!("cargo:rerun-if-changed={}", source.display());
    let script = std::fs::read_to_string(&source).unwrap_or_default();
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    std::fs::write(Path::new(&out_dir).join("join.sh"), script)
        .expect("write the embedded join script");
}
