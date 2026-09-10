//! The real machinery both convergence stories run against: a built Stado
//! dashboard, a real isolated Skarbiec vault, the grants a client presents,
//! and the helpers that read what came back.

use std::fs::{self, File};
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};


use serde_json::{json, Value};


use crate::helpers::{
    binary_version, hostname, mint_verifier, next_patch_version, release_platform, stage_current,
    unused_loopback_port,
};
use crate::skarbiec_support::{real_skarbiec_binary, SkarbiecFixture, SkarbiecItem};

pub const HOST: &str = "probierz-service-convergence-host";
pub const PATH_ENV: &str = "/usr/bin:/bin:/usr/sbin:/sbin";


pub(crate) struct ClientGrant {
    pub(crate) name: String,
    actions: Vec<&'static str>,
    pub(crate) bearer: String,
}

impl ClientGrant {
    pub(crate) fn new(name: &str, actions: Vec<&'static str>, bearer: String) -> Self {
        Self {
            name: name.to_string(),
            actions,
            bearer,
        }
    }

    pub(crate) fn item(&self) -> String {
        format!("{}-registry-api", self.name)
    }
}

pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

pub(crate) struct DashboardFixture {
    pub(crate) _root: tempfile::TempDir,
    pub(crate) home: PathBuf,
    pub(crate) storage: PathBuf,
    pub(crate) config: PathBuf,
    pub(crate) protected: PathBuf,
    pub(crate) stado: PathBuf,
    pub(crate) skarbiec: PathBuf,
    pub(crate) skarbiec_current: String,
    pub(crate) skarbiec_declared: String,
    pub(crate) address: SocketAddr,
    pub(crate) vault: Option<SkarbiecFixture>,
    pub(crate) verifier_mint: Option<Value>,
    pub(crate) verifier_capabilities: Option<String>,
    pub(crate) dashboard: Child,
}

impl DashboardFixture {
    pub(crate) fn start(clients: &[ClientGrant]) -> Self {
        let runs = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/service-convergence-runs");
        fs::create_dir_all(&runs).expect("service-convergence run root");
        let root = tempfile::Builder::new()
            .prefix("stado-service-convergence-")
            .tempdir_in(runs)
            .expect("repo-rooted service-convergence fixture");
        let home = root.path().join("home");
        let storage = root.path().join("storage");
        let config = root.path().join("stado-config.json");
        let skarbiec_home = root.path().join("skarbiec-home");
        for directory in [
            &home,
            &storage,
            &skarbiec_home,
            &home.join("tmp"),
            &home.join(".stado/bin"),
            &home.join(".stado/protected"),
        ] {
            fs::create_dir_all(directory).expect("isolated fixture directory");
        }
        let verifier_token = home.join(".stado/registry-api-verifier-grant");
        let port = unused_loopback_port();
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        fs::write(
            &config,
            serde_json::to_vec_pretty(&json!({
                "api": {"url": format!("http://{address}")},
                "storage": {"stado": {"url": format!("http://{address}")}}
            }))
            .expect("isolated Stado configuration JSON"),
        )
        .expect("isolated Stado configuration");

        let stado = home.join(".stado/bin/stado");
        let skarbiec = home.join(".stado/bin/skarbiec");
        fs::copy(env!("CARGO_BIN_EXE_stado"), &stado).expect("copy built Stado binary");
        fs::copy(real_skarbiec_binary(), &skarbiec).expect("copy real Skarbiec binary");
        for binary in [&stado, &skarbiec] {
            fs::set_permissions(binary, fs::Permissions::from_mode(0o700))
                .expect("fixture binary is executable");
        }
        let skarbiec_current = binary_version(&skarbiec, &home);
        let skarbiec_declared = next_patch_version(&skarbiec_current);
        stage_current(&home, "stado", env!("CARGO_PKG_VERSION"), &stado);
        stage_current(&home, "skarbiec", &skarbiec_current, &skarbiec);
        let protected = home.join(".stado/protected/operator-state.json");
        fs::write(
            &protected,
            b"{\"owner\":\"probierz\",\"must_survive_failed_delivery\":true}\n",
        )
        .expect("protected fixture state");

        let hostname = hostname();
        let short_hostname = hostname.trim_end_matches(".local").to_string();
        let registry = json!({
            "schema_version": 2,
            "targets": [{
                "name": HOST,
                "kind": "local",
                "ssh": null,
                "release_platform": release_platform(),
                "hostnames": [hostname, short_hostname],
                "role": "interactive",
                "managed_versions": {
                    "skarbiec": skarbiec_declared,
                    "stado": env!("CARGO_PKG_VERSION")
                },
                "services": []
            }],
            "coordinators": []
        });
        fs::write(
            storage.join("registry.json"),
            serde_json::to_vec_pretty(&registry).expect("registry JSON"),
        )
        .expect("isolated registry");

        let (vault, verifier_mint, verifier_capabilities) = if clients.is_empty() {
            (None, None, None)
        } else {
            let items = clients
                .iter()
                .map(|client| {
                    SkarbiecItem::new(
                        client.item(),
                        "token",
                        json!({
                            "schema": "skarbiec.item.v2",
                            "kind": "token",
                            "fields": {"token": client.bearer},
                            "context": {"service": "stado-registry-api", "client": client.name}
                        }),
                    )
                })
                .collect::<Vec<_>>();
            let capabilities = clients
                .iter()
                .map(|client| format!("read:{}#token", client.item()))
                .collect::<Vec<_>>()
                .join(",");
            let mut receipt = None;
            let vault = SkarbiecFixture::start(
                &skarbiec_home,
                &items,
                verifier_token.clone(),
                None,
                |gnupg_home, vault_file| {
                    let minted = mint_verifier(
                        &home,
                        &storage,
                        &config,
                        gnupg_home,
                        vault_file,
                        &capabilities,
                        "registry-api-verifier-grant",
                    );
                    assert!(
                        minted.status.success(),
                        "built Stado failed to provision the verifier bearer: {}",
                        String::from_utf8_lossy(&minted.stderr)
                    );
                    receipt = Some(
                        serde_json::from_slice(&minted.stdout)
                            .expect("verifier bearer receipt from built Stado is JSON"),
                    );
                },
            );
            (Some(vault), receipt, Some(capabilities))
        };

        let client_document = clients
            .iter()
            .map(|client| {
                (
                    client.name.clone(),
                    json!({"item": client.item(), "actions": client.actions}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let stdout =
            File::create(root.path().join("dashboard.stdout.log")).expect("dashboard stdout log");
        let stderr =
            File::create(root.path().join("dashboard.stderr.log")).expect("dashboard stderr log");
        let mut dashboard_command = Command::new(env!("CARGO_BIN_EXE_stado"));
        dashboard_command
            .args([
                "dashboard",
                "--bind",
                "127.0.0.1",
                "--port",
                &port.to_string(),
            ])
            .env_clear()
            .env("HOME", &home)
            .env("PATH", PATH_ENV)
            .env("TMPDIR", home.join("tmp"))
            .env("STADO_CONFIG", &config)
            .env("STADO_API_URL", format!("http://{address}"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &storage)
            .env("WC_STADO_STORAGE_NAMESPACE", "service-convergence")
            .env(
                "WC_REGISTRY_API_CLIENTS",
                Value::Object(client_document).to_string(),
            )
            .env("WC_DASHBOARD_BOUNDARY_ATTEMPTS", "1")
            .env("WC_DASHBOARD_BOUNDARY_TIMEOUT_SECONDS", "10")
            .env("WC_DASHBOARD_TRUST_HTTPS_PROXY", "true")
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr);
        if let Some(vault) = &vault {
            dashboard_command
                .env("WC_REGISTRY_SKARBIEC_URL", vault.url())
                .env(
                    "WC_REGISTRY_SKARBIEC_CONSUMER",
                    "stado-registry-api-verifier",
                )
                .env("WC_REGISTRY_SKARBIEC_TOKEN_FILE", &vault.token);
        }
        let dashboard = dashboard_command
            .spawn()
            .expect("built Stado dashboard starts");
        let mut fixture = Self {
            _root: root,
            home,
            storage,
            config,
            protected,
            stado,
            skarbiec,
            skarbiec_current,
            skarbiec_declared,
            address,
            vault,
            verifier_mint,
            verifier_capabilities,
            dashboard,
        };
        fixture.wait_until_ready();
        fixture
    }
}



impl Drop for DashboardFixture {
    fn drop(&mut self) {
        let _ = self.dashboard.kill();
        let _ = self.dashboard.wait();
        drop(self.vault.take());
    }
}

