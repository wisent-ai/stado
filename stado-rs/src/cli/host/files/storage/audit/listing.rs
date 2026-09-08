/// The namespace-inventory listing of [`super::backup_audit`].
pub(super) fn print_inventory_objects(audit: &crate::deploy::host_backup_audit::BackupAudit) {
    println!(
        "namespace inventory: complete={} local-storage=[{}] local-backup=[{}]",
        audit.complete,
        audit
            .namespaces
            .get("local_storage")
            .map(|names| names.join(","))
            .unwrap_or_default(),
        audit
            .namespaces
            .get("local_backup")
            .map(|names| names.join(","))
            .unwrap_or_default(),
    );
    for object in &audit.inventory_objects {
        println!(
            "{} local-storage={}({}) local-backup={}({}) metadata={}/{}",
            object.path,
            object.primary.state,
            object
                .primary
                .bytes
                .map_or_else(|| "-".to_string(), |bytes| bytes.to_string()),
            object.backup.state,
            object
                .backup
                .bytes
                .map_or_else(|| "-".to_string(), |bytes| bytes.to_string()),
            object.primary_metadata.state,
            object.backup_metadata.state,
        );
    }
}

/// The exact-object listing of [`super::backup_audit`].
pub(super) fn print_exact_objects(audit: &crate::deploy::host_backup_audit::BackupAudit) {
    println!(
        "namespaces: complete={} local-storage=[{}] local-backup=[{}]",
        audit.namespace_inventory_complete,
        audit
            .namespaces
            .get("local_storage")
            .map(|names| names.join(","))
            .unwrap_or_default(),
        audit
            .namespaces
            .get("local_backup")
            .map(|names| names.join(","))
            .unwrap_or_default(),
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
                identity
                    .bytes
                    .map_or_else(|| "-".to_string(), |bytes| bytes.to_string()),
                identity.sha256.as_deref().unwrap_or("-"),
            );
        }
        println!(
            "  metadata: local-storage state={} bytes={} sha256={}; local-backup state={} bytes={} sha256={}",
            object.primary_metadata.state,
            object
                .primary_metadata
                .bytes
                .map_or_else(|| "-".to_string(), |bytes| bytes.to_string()),
            object.primary_metadata.sha256.as_deref().unwrap_or("-"),
            object.backup_metadata.state,
            object
                .backup_metadata
                .bytes
                .map_or_else(|| "-".to_string(), |bytes| bytes.to_string()),
            object.backup_metadata.sha256.as_deref().unwrap_or("-"),
        );
    }
}
