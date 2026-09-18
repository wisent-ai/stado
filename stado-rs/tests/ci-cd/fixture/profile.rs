use super::*;

pub(super) fn configure(home: &Path, storage: &Path, broker_url: &str) {
    // Native delivery workers intentionally do not inherit control-plane env
    // overrides. Give this isolated host its real persisted deployment profile.
    // The build job inherits no `WC_*` variable either, so the broker it
    // reads the signing identity from is declared here, the way a fleet
    // builder declares its own; without it the job asked the default
    // loopback port and got the operator's real broker's 403.
    run(Command::new(env!("CARGO_BIN_EXE_stado"))
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var("PATH").unwrap())
        .args(["config", "init"]));
    for (key, value) in [
        ("storage.local.path", storage.to_str().unwrap()),
        ("storage.stado.namespace", "ci-release"),
        ("secrets.skarbiec.url", broker_url),
    ] {
        run(Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("HOME", home)
            .env("PATH", std::env::var("PATH").unwrap())
            .env("STADO_CONFIG", home.join(".stado/config.json"))
            .args(["config", "set", key, value]));
    }
}
