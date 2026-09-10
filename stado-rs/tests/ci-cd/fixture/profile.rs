use super::*;

pub(super) fn configure(home: &Path, storage: &Path) {
    // Native delivery workers intentionally do not inherit control-plane env
    // overrides. Give this isolated host its real persisted deployment profile.
    run(Command::new(env!("CARGO_BIN_EXE_stado"))
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var("PATH").unwrap())
        .args(["config", "init"]));
    for (key, value) in [
        ("storage.local.path", storage.to_str().unwrap()),
        ("storage.stado.namespace", "ci-release"),
    ] {
        run(Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("HOME", home)
            .env("PATH", std::env::var("PATH").unwrap())
            .env("STADO_CONFIG", home.join(".stado/config.json"))
            .args(["config", "set", key, value]));
    }
}
