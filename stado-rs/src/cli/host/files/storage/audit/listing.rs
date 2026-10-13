//! The listings of [`super::backup_audit`]. A size, digest or namespace list
//! the audit did not measure is said to be so: a dash read as a value, and
//! an empty bracket as a namespace that holds nothing.

use crate::deploy::host_backup_audit::BackupAudit;

fn bytes_text(bytes: Option<u64>) -> String {
    match bytes {
        Some(bytes) => bytes.to_string(),
        None => "not measured".to_string(),
    }
}

fn digest_text(sha256: Option<&str>) -> &str {
    match sha256 {
        Some(sha256) => sha256,
        None => "not measured",
    }
}

fn namespace_text(audit: &BackupAudit, namespace: &str) -> String {
    match audit.namespaces.get(namespace) {
        Some(names) => names.join(","),
        None => "not listed".to_string(),
    }
}

/// The namespace-inventory listing of [`super::backup_audit`].
pub(super) fn print_inventory_objects(audit: &BackupAudit) {
    println!(
        "namespace inventory: complete={} local-storage=[{}] local-backup=[{}]",
        audit.complete,
        namespace_text(audit, "local_storage"),
        namespace_text(audit, "local_backup"),
    );
    for object in &audit.inventory_objects {
        println!(
            "{} local-storage={}({}) local-backup={}({}) metadata={}/{}",
            object.path,
            object.primary.state,
            bytes_text(object.primary.bytes),
            object.backup.state,
            bytes_text(object.backup.bytes),
            object.primary_metadata.state,
            object.backup_metadata.state,
        );
    }
}

/// The exact-object listing of [`super::backup_audit`].
pub(super) fn print_exact_objects(audit: &BackupAudit) {
    println!(
        "namespaces: complete={} local-storage=[{}] local-backup=[{}]",
        audit.namespace_inventory_complete,
        namespace_text(audit, "local_storage"),
        namespace_text(audit, "local_backup"),
    );
    for object in &audit.objects {
        println!("{}", object.path);
        for (name, identity) in [
            ("local-storage", &object.primary),
            ("local-backup", &object.backup),
        ] {
            println!(
                "  {name}: state={} bytes={} sha256={}",
                identity.state,
                bytes_text(identity.bytes),
                digest_text(identity.sha256.as_deref()),
            );
        }
        println!(
            "  metadata: local-storage state={} bytes={} sha256={}; local-backup state={} bytes={} sha256={}",
            object.primary_metadata.state,
            bytes_text(object.primary_metadata.bytes),
            digest_text(object.primary_metadata.sha256.as_deref()),
            object.backup_metadata.state,
            bytes_text(object.backup_metadata.bytes),
            digest_text(object.backup_metadata.sha256.as_deref()),
        );
    }
}
