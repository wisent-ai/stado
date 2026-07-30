//! Tests for the credential store selector. STADO_CREDENTIAL_STORE is
//! process-global, so every test serializes through one mutex and restores
//! the variable on drop. File modes are applied with `chmod` so no numeric
//! literal appears in source.

use super::*;
use std::sync::{Mutex, MutexGuard};

fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
    LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct StoreEnv;

impl StoreEnv {
    fn set(value: &str) -> Self {
        std::env::set_var(ENV_STORE, value);
        Self
    }
    fn unset() -> Self {
        std::env::remove_var(ENV_STORE);
        Self
    }
}

impl Drop for StoreEnv {
    fn drop(&mut self) {
        std::env::remove_var(ENV_STORE);
    }
}

fn write_store(contents: &str, mode: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("creds.json");
    std::fs::write(&path, contents).expect("write store");
    let status = std::process::Command::new("chmod")
        .arg(mode)
        .arg(&path)
        .status()
        .expect("chmod store");
    assert!(status.success());
    (dir, path)
}

#[test]
fn unset_selects_skarbiec() {
    let _guard = env_lock();
    let _env = StoreEnv::unset();
    assert_eq!(selected().expect("selected"), Backend::Skarbiec { url: None });
}

#[test]
fn bare_skarbiec_selects_skarbiec() {
    let _guard = env_lock();
    let _env = StoreEnv::set("skarbiec");
    assert_eq!(selected().expect("selected"), Backend::Skarbiec { url: None });
}

#[test]
fn skarbiec_url_override_is_carried() {
    let _guard = env_lock();
    let _env = StoreEnv::set("skarbiec://https://vault.example.com");
    assert_eq!(
        selected().expect("selected"),
        Backend::Skarbiec {
            url: Some("https://vault.example.com".to_string()),
        }
    );
}

#[test]
fn unsupported_scheme_is_a_hard_error() {
    let _guard = env_lock();
    for (raw, scheme) in [
        ("http://example.com/vault", "http"),
        ("https://example.com", "https"),
        ("env://HOME", "env"),
        ("gibberish", "gibberish"),
    ] {
        let _env = StoreEnv::set(raw);
        match selected() {
            Err(SkarbiecError::Deployment(detail)) => {
                assert!(detail.contains("unsupported credential store"), "{detail}");
                assert!(detail.contains(scheme), "{detail}");
            }
            other => panic!("{raw:?} must be rejected, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn file_backend_reads_values() {
    let _guard = env_lock();
    let (_dir, path) = write_store(
        r#"{"stado-vast": {"api_key": "vast-key"}, "stado-box": {"api_key": "box-key", "note": "n"}}"#,
        "600",
    );
    let _env = StoreEnv::set(&format!("file://{}", path.display()));
    assert_eq!(
        read_string("stado-vast", "api_key").await.expect("read"),
        Some("vast-key".to_string())
    );
    let item = read_item("stado-box").await.expect("item");
    assert_eq!(item.get("note").and_then(Value::as_str), Some("n"));
    // A bare absolute path selects the same backend.
    std::env::set_var(ENV_STORE, path.display().to_string());
    assert_eq!(
        read_string("stado-vast", "api_key").await.expect("read"),
        Some("vast-key".to_string())
    );
}

#[tokio::test]
async fn file_backend_missing_item_and_field() {
    let _guard = env_lock();
    let (_dir, path) = write_store(r#"{"stado-vast": {"api_key": "vast-key"}}"#, "600");
    let _env = StoreEnv::set(&format!("file://{}", path.display()));
    match read_item("stado-missing").await {
        Err(SkarbiecError::MissingValue(id)) => assert_eq!(id, "stado-missing"),
        other => panic!("missing item must be MissingValue, got {other:?}"),
    }
    assert_eq!(
        read_string("stado-missing", "api_key").await.expect("read"),
        None
    );
    assert_eq!(
        read_string("stado-vast", "no_such").await.expect("read"),
        None
    );
}

#[tokio::test]
async fn file_backend_refuses_group_readable_file() {
    let _guard = env_lock();
    let (_dir, path) = write_store(r#"{"stado-vast": {"api_key": "vast-key"}}"#, "644");
    let _env = StoreEnv::set(&format!("file://{}", path.display()));
    match read_string("stado-vast", "api_key").await {
        Err(SkarbiecError::Deployment(_)) => {}
        other => panic!("group-readable store must be refused, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn file_backend_refuses_symlink() {
    let _guard = env_lock();
    let (_dir, path) = write_store(r#"{"stado-vast": {"api_key": "vast-key"}}"#, "600");
    let link = path.with_extension("link.json");
    std::os::unix::fs::symlink(&path, &link).expect("symlink");
    let _env = StoreEnv::set(&format!("file://{}", link.display()));
    match read_string("stado-vast", "api_key").await {
        Err(SkarbiecError::Deployment(_)) => {}
        other => panic!("symlink store must be refused, got {other:?}"),
    }
}
