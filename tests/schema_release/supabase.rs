//! The Supabase engine with the real Supabase CLI release Stado declares,
//! fetched from its publisher into the run's own home, and the host's real
//! Docker daemon.

use crate::fixture::Run;
use crate::CREATE;
use serde_json::Value;
use std::fs;

/// The Supabase engine installs the CLI Stado declares into the run's own
/// home on first use and runs the bundle's migrations with it. Where the
/// local stack cannot start, the refusal is the CLI's own about Docker, never
/// a missing CLI.
#[test]
fn supabase_verify_installs_the_declared_cli() {
    let mut run = Run::new();
    let project = run.source.join("supabase");
    fs::create_dir_all(project.join("migrations")).unwrap();
    fs::write(
        project.join("config.toml"),
        "project_id = \"schema-release-journey\"\n",
    )
    .unwrap();
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
    fs::write(
        project
            .join("migrations")
            .join(format!("{stamp}_create.sql")),
        CREATE,
    )
    .unwrap();
    let mut command = run.stado();
    command.args([
        "product",
        "source-bundle",
        "--name",
        "supabase-source.tar",
        "--include",
        "supabase/config.toml",
        "--include",
        "supabase/migrations",
    ]);
    run.success(command);
    let mut command = run.stado();
    command.args(["product", "schema", "verify", "--engine", "supabase"]);
    let verified = run.run(command);
    let declared: Value = serde_json::from_str(include_str!(
        "../../stado-rs/data/declarations/supabase-cli.json"
    ))
    .unwrap();
    let version = declared["version"].as_str().unwrap();
    let binary = run
        .root
        .join("home/.local/share/stado-supabase")
        .join(version)
        .join("supabase");
    let mut command = run.command(&binary);
    command.arg("--version");
    assert_eq!(run.success(command).trim(), version);
    let stderr = String::from_utf8_lossy(&verified.stderr);
    if verified.status.success() {
        let stdout = String::from_utf8_lossy(&verified.stdout);
        assert!(stdout.contains("applied to a scratch database"), "{stdout}");
    } else {
        assert!(stderr.to_lowercase().contains("docker"), "{stderr}");
        assert!(!stderr.contains("No such file or directory"), "{stderr}");
    }
    run.pass();
}
