use crate::targets::*;

/// Record a document the authority served AND this build validated.
///
/// The gate is the registry-v2 contract ([`validate_registry`]), not the
/// loader's tolerance. [`load_registry_from_str`] SKIPS what it cannot model:
/// a `targets` value that is the string `"not-a-list"` leaves it holding zero
/// targets and no complaint, so gating the copy on the loader alone recorded
/// that string as the fleet's last known good registry and then served it —
/// an empty fleet — for as long as the store stayed down. Confirmed by hand
/// on 2026-08-19 against this exact code before the gate moved here.
///
/// A document that fails the contract is reported and not recorded: the copy
/// already on disk is worth more than the newest thing the store happened to
/// hold, and a cache nobody can trust is a cache nobody may use. The refusal
/// is RETURNED as well as printed ([`LastGoodRefusal`]): a caller that cannot
/// see it cannot tell an operator the host stopped taking copies, which is
/// how a host sits a generation behind with one stderr line as the evidence.
///
/// Document first, sidecar second. A crash between the two leaves a new
/// document dated by the older sidecar, which OVERSTATES the age; the other
/// order understates it, and a registry that reads fresher than it is, is
/// the exact lie this cache exists to prevent.
pub fn store_last_good(text: &str, generation: &str) -> Result<(), LastGoodRefusal> {
    let (Some(document), Some(meta)) = (registry_last_good_path(), registry_last_good_meta_path())
    else {
        // Nothing to name in a sentence, so nothing is printed — the caller
        // gets the outcome instead.
        return Err(LastGoodRefusal::NoCacheLocation);
    };
    let report = |error: &dyn std::fmt::Display| {
        eprintln!(
            "[registry-cache] not recording the last-known-good registry in {}: {error}",
            document.display()
        );
    };
    // Every arm below builds the refusal, prints the same sentence it always
    // printed (`Display` is the underlying detail verbatim), and returns it.
    let refuse = |refusal: LastGoodRefusal| -> Result<(), LastGoodRefusal> {
        report(&refusal);
        Err(refusal)
    };
    match serde_json::from_str::<Value>(text) {
        Ok(data) => {
            if let Err(error) = validate_registry(&data) {
                return refuse(LastGoodRefusal::RejectedByThisBuild {
                    detail: error.to_string(),
                });
            }
            // Valid is not the same as better. A document naming no hosts
            // never replaces one that names some.
            //
            // Measured on 2026-09-01: a forced push replaced the canonical
            // registry with a 65-byte skeleton, and seventeen minutes later
            // this cache - the product's own recovery path - recorded that
            // skeleton as the last KNOWN GOOD registry, destroying the one
            // copy it exists to provide. Recovery came from an operator's
            // private snapshot instead.
            //
            // The rule is relative rather than an absolute "never cache an
            // empty document" floor, and that is deliberate: a fresh install
            // legitimately declares no targets and must still be cacheable.
            // What is refused is LOSING hosts, not being empty.
            let recorded = std::fs::read_to_string(&document)
                .ok()
                .and_then(|held| serde_json::from_str::<Value>(&held).ok());
            if let Err(error) = may_replace_last_good(&data, recorded.as_ref()) {
                return refuse(LastGoodRefusal::WouldLoseRecordedHosts {
                    held: recorded.as_ref().map_or(0, declared_target_count),
                    detail: error,
                });
            }
        }
        Err(error) => {
            return refuse(LastGoodRefusal::Unparseable {
                detail: error.to_string(),
            });
        }
    }
    if let Some(directory) = document.parent() {
        if let Err(error) = std::fs::create_dir_all(directory) {
            return refuse(LastGoodRefusal::NotWritten {
                detail: error.to_string(),
            });
        }
    }
    let sidecar = serde_json::to_string(&RegistryCacheMeta {
        read_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        generation: generation.to_string(),
    })
    .expect("registry cache metadata serialization is infallible");
    if let Err(error) = write_atomic(&document, text) {
        return refuse(LastGoodRefusal::NotWritten {
            detail: error.to_string(),
        });
    }
    // The document landed and the sidecar did not: a copy nobody can date,
    // which `load_last_good` skips. Still a refusal to the caller, and the
    // policy is untouched — this was already the last statement.
    if let Err(error) = write_atomic(&meta, &sidecar) {
        return refuse(LastGoodRefusal::NotWritten {
            detail: error.to_string(),
        });
    }
    Ok(())
}

/// The cached document with the age of what it is, or `None` when there is
/// no usable pair on disk.
///
/// Both files or neither: the age is part of the answer, and a document
/// nobody can date is indistinguishable from a document from last year. The
/// contract was checked on the way in ([`store_last_good`]); on the way out
/// the loader is the gate, so a copy truncated by a full disk is skipped
/// rather than served as an empty fleet.
pub(crate) fn load_last_good() -> Option<(Registry, RegistryCacheMeta, i64)> {
    let meta: RegistryCacheMeta =
        serde_json::from_str(&std::fs::read_to_string(registry_last_good_meta_path()?).ok()?)
            .ok()?;
    let read_at = DateTime::parse_from_rfc3339(&meta.read_at)
        .ok()?
        .with_timezone(&Utc);
    let text = std::fs::read_to_string(registry_last_good_path()?).ok()?;
    let mut registry = load_registry_from_str(&text).ok()?;
    let age = (Utc::now() - read_at).num_seconds().max(0);
    registry.staleness_seconds = Some(age);
    Some((registry, meta, age))
}

/// Whether this process has already told the operator it is reading a copy.
static REGISTRY_NOTICE_REPORTED: AtomicBool = AtomicBool::new(false);

/// Print a fallback notice to stderr, once per process.
///
/// Once, because a fleet sweep resolves twenty hosts through one dead
/// authority: twenty copies of the same sentence bury the twenty answers the
/// operator asked for, and the sentence is about the process, not the host.
pub fn report_registry_notice(notice: &str) {
    if !REGISTRY_NOTICE_REPORTED.swap(true, Ordering::Relaxed) {
        eprintln!("{notice}");
    }
}
