/// The classification, free-space and reclaim summary of
/// [`super::backup_audit`].
pub(super) fn print_classes(
    audit: &crate::deploy::host_backup_audit::BackupAudit,
    reclaim_twins: bool,
    apply: bool,
    gib: &dyn Fn(u64) -> f64,
) {
    for class in [
        crate::deploy::host_backup_audit::TWIN,
        crate::deploy::host_backup_audit::ABSENT,
        crate::deploy::host_backup_audit::DIFFERS,
        crate::deploy::host_backup_audit::SAME_SIZE_UNPROVEN,
    ] {
        let totals = audit.classes.get(class).cloned().unwrap_or_default();
        println!(
            "{class:9} {:>7} object(s)  {:>8.2} GiB",
            totals.objects,
            gib(totals.bytes)
        );
        for (bytes, path) in audit.examples.get(class).into_iter().flatten() {
            println!("          {:>8.2} GiB  {path}", gib(*bytes));
        }
    }
    println!(
        "reclaim:  {:.2} GiB proven present and identical in the primary; {:.2} GiB is data and stays",
        gib(audit.reclaimable_bytes()),
        gib(audit.retained_bytes())
    );
    // The free-space pair the pass read itself, on both sides of its own
    // deletions. Reported even for a read-only classification, because "how
    // full is this disk while you are telling me what is on it" is the
    // question the whole command exists to serve.
    let gib_kb = |blocks: i64| blocks as f64 / 1024.0 / 1024.0;
    if let (Some(before), Some(after)) = (audit.free_kb_before, audit.free_kb_after) {
        println!(
            "free:     {:.2} GiB before, {:.2} GiB after ({:+.2} GiB)",
            gib_kb(before),
            gib_kb(after),
            gib_kb(after - before),
        );
    }
    if reclaim_twins {
        if apply {
            println!(
                "deleted:  {} object(s)  {:.2} GiB, each one hashed on both sides by this pass; \
                 {} emptied directories removed",
                audit.deleted.objects,
                gib(audit.deleted.bytes),
                audit.pruned_directories,
            );
            if audit.delete_failed.objects > 0 {
                println!(
                    "refused:  {} object(s) the host would not unlink; they are still in the replica",
                    audit.delete_failed.objects
                );
            }
            if !audit.reclaim_complete {
                println!(
                    "warning:  the reclaim half did not print its own end marker, so the deleted \
                     count is a floor; run the command again"
                );
            }
        } else {
            println!(
                "would delete: {} object(s)  {:.2} GiB — nothing was changed; pass --apply, which \
                 re-proves every one of them in that same pass",
                audit.would_delete.objects,
                gib(audit.would_delete.bytes),
            );
        }
    }
    if !audit.complete {
        println!(
            "warning:  the host did not finish classifying, so these totals are a floor, not the answer"
        );
    }
}
