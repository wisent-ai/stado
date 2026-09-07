use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Fixture {
    root: tempfile::TempDir,
    host: String,
    vault: PathBuf,
    registry_before: String,
}

impl Fixture {
    fn new(declared: bool) -> Self {
        let root = tempfile::tempdir().expect("temp storage");
        let host = String::from_utf8(
            Command::new("hostname")
                .output()
                .expect("hostname runs")
                .stdout,
        )
        .expect("hostname is UTF-8")
        .trim()
        .to_string();
        let registry_before = serde_json::json!({
            "schema_version": 2,
            "targets": [{
                "name": host,
                "kind": "local",
                "release_platform": "darwin-arm64",
                "hostnames": [host],
            }],
            "coordinators": [],
        })
        .to_string();
        std::fs::write(root.path().join("registry.json"), &registry_before).unwrap();

        let bin = root.path().join(".stado/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let vault = root.path().join("declared/owner.vault.json");
        std::fs::create_dir_all(vault.parent().unwrap()).unwrap();
        std::fs::write(&vault, r#"{"owner":"test-owner","items":{}}"#).unwrap();

        let config = bin.join("stado");
        let declaration = if declared {
            vault.to_string_lossy().into_owned()
        } else {
            String::new()
        };
        std::fs::write(
            &config,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{}'\n",
                serde_json::json!({"resolved": {"skarbiec_vault_file": declaration}})
            ),
        )
        .unwrap();
        executable(&config);

        let skarbiec = bin.join("skarbiec");
        std::fs::write(
            &skarbiec,
            r#"#!/bin/sh
{
  printf 'vault=%s argv=' "$SKARBIEC_VAULT_FILE"
  printf ' <%s>' "$@"
  printf '\n'
} >> "$STADO_TEST_ARGV_LOG"
case "$1" in
  tokens) printf '[]\n' ;;
  token-mint) printf '{"ok":true,"consumer":"desktop","capabilities":[],"token":"secret-issued-bearer"}\n' ;;
  set-json) cat >/dev/null; printf '{}\n' ;;
  list) printf '[]\n' ;;
  vaults) printf '{"host":"fixture","vaults":[]}\n' ;;
  *) printf '{}\n' ;;
esac
"#,
        )
        .unwrap();
        executable(&skarbiec);

        Self {
            root,
            host,
            vault,
            registry_before,
        }
    }

    fn run(&self, args: &[&str], stdin: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .current_dir(self.root.path())
            .env("HOME", self.root.path())
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.path())
            .env("STADO_CONFIG", self.root.path().join("no-such-config.json"))
            .env("STADO_TEST_ARGV_LOG", self.root.path().join("argv.log"))
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("WC_PROFILES_DIR")
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("stado runs");
        if let Some(body) = stdin {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(body.as_bytes())
                .unwrap();
        }
        child.wait_with_output().expect("stado finishes")
    }

    fn unchanged(&self) {
        assert_eq!(
            std::fs::read_to_string(self.root.path().join("registry.json")).unwrap(),
            self.registry_before,
            "credential reads and refusals must not mutate the declaration"
        );
    }
}

fn executable(path: &Path) {
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(path, permissions).unwrap();
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn credentials_help_exposes_the_moved_capability_and_host_help_does_not() {
    let fixture = Fixture::new(true);
    let credentials = fixture.run(&["credentials", "--help"], None);
    assert!(
        credentials.status.success(),
        "{}",
        text(&credentials.stderr)
    );
    let help = text(&credentials.stdout);
    for operation in [
        "item",
        "token",
        "vaults",
        "vault",
        "acquisition-scopes",
        "grant",
        "backup",
        "seed-freshness",
    ] {
        assert!(
            help.contains(operation),
            "credentials help omitted {operation}: {help}"
        );
    }
    for (path, operations) in [
        (
            &["credentials", "item", "--help"][..],
            &["put", "show", "retag"][..],
        ),
        (&["credentials", "token", "--help"][..], &["mint"][..]),
        (&["credentials", "vault", "--help"][..], &["sync"][..]),
        (
            &["credentials", "acquisition-scopes", "--help"][..],
            &["sync"][..],
        ),
        (
            &["credentials", "grant", "--help"][..],
            &["item-read", "show"][..],
        ),
        (&["credentials", "backup", "--help"][..], &["audit"][..]),
    ] {
        let output = fixture.run(path, None);
        assert!(output.status.success(), "{}", text(&output.stderr));
        let help = text(&output.stdout);
        for operation in operations {
            assert!(
                help.contains(operation),
                "{} help omitted {operation}: {help}",
                path.join(" ")
            );
        }
    }

    let host = fixture.run(&["host", "--help"], None);
    assert!(host.status.success(), "{}", text(&host.stderr));
    let help = text(&host.stdout);
    for removed in [
        "vault-item-put",
        "vault-item-show",
        "vault-token-mint",
        "vaults",
        "retag-vault-item",
        "sync-vault",
        "sync-acquisition-scopes",
        "grant-item-read",
        "grant-show",
        "backup-audit",
        "authenticator-seed-freshness",
    ] {
        assert!(
            !help.contains(removed),
            "host help retained {removed}: {help}"
        );
    }
}

#[test]
fn vault_authority_is_resolved_from_the_host_declaration() {
    let fixture = Fixture::new(true);
    let output = fixture.run(
        &[
            "credentials",
            "token",
            "mint",
            "--host",
            &fixture.host,
            "desktop",
            "--capabilities",
            "read:item#field",
            "--audience",
            "desktop",
            "--json",
        ],
        None,
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    let stdout = text(&output.stdout);
    assert!(
        !stdout.contains("secret-issued-bearer"),
        "bearer leaked to stdout: {stdout}"
    );
    let log = std::fs::read_to_string(fixture.root.path().join("argv.log")).unwrap();
    assert!(
        log.contains(&format!("vault={}", fixture.vault.display())),
        "declared vault was not selected: {log}"
    );
    fixture.unchanged();
}

#[test]
fn host_without_a_vault_authority_is_refused_by_sentence() {
    let fixture = Fixture::new(false);
    let output = fixture.run(
        &[
            "credentials",
            "grant",
            "show",
            "--host",
            &fixture.host,
            "desktop",
        ],
        None,
    );
    assert!(!output.status.success());
    assert!(
        text(&output.stderr).contains(&format!(
            "{} declares no vault authority; add it to secrets.skarbiec.vault_file",
            fixture.host
        )),
        "{}",
        text(&output.stderr)
    );
    fixture.unchanged();
}

#[test]
fn unknown_item_and_absent_grant_are_refused_by_sentences() {
    let fixture = Fixture::new(true);
    let item = fixture.run(
        &[
            "credentials",
            "item",
            "show",
            "--host",
            &fixture.host,
            "missing-item",
        ],
        None,
    );
    assert!(!item.status.success());
    assert!(
        text(&item.stderr).contains(&format!("{} declares no credential item missing-item; add it to the vault declared by secrets.skarbiec.vault_file", fixture.host)),
        "{}",
        text(&item.stderr)
    );

    let grant = fixture.run(
        &[
            "credentials",
            "grant",
            "show",
            "--host",
            &fixture.host,
            "missing-consumer",
        ],
        None,
    );
    assert!(!grant.status.success());
    assert!(
        text(&grant.stderr).contains(&format!("{} declares no grant for missing-consumer; add it to the vault declared by secrets.skarbiec.vault_file", fixture.host)),
        "{}",
        text(&grant.stderr)
    );
    fixture.unchanged();
}

#[test]
fn item_put_keeps_secret_values_off_stdout_and_the_remote_argument_vector() {
    let fixture = Fixture::new(true);
    let secret = "never-appears-in-argv-4f5f9d";
    let payload = serde_json::json!({"kind":"login", "fields":{"password":secret}}).to_string();
    let output = fixture.run(
        &[
            "credentials",
            "item",
            "put",
            "--host",
            &fixture.host,
            "login-item",
            "--type",
            "login",
        ],
        Some(&payload),
    );
    assert!(
        !output.status.success(),
        "the inert fixture unexpectedly persisted the item"
    );
    assert!(
        !text(&output.stdout).contains(secret),
        "secret leaked to stdout"
    );
    assert!(
        !text(&output.stderr).contains(secret),
        "secret leaked to stderr"
    );
    let rendered_remote_command =
        std::fs::read_to_string(fixture.root.path().join("argv.log")).unwrap();
    assert!(rendered_remote_command.contains("<set-json> <login-item> <--type> <login>"));
    assert!(
        !rendered_remote_command.contains(secret),
        "secret entered the remote argument vector: {rendered_remote_command}"
    );
    fixture.unchanged();
}
