use crate::targets::*;

// ---------------------------------------------------------------------------
// last-known-good cache — what a reader answers with when the store is down
// ---------------------------------------------------------------------------

/// Reader-side copy of the last registry document the authority served.
pub const REGISTRY_LAST_GOOD_FILE: &str = "registry-last-good.json";
/// Sidecar dating [`REGISTRY_LAST_GOOD_FILE`] and naming which document it
/// is. Kept beside the copy rather than inside it so the copy stays
/// byte-identical to what the authority served.
pub const REGISTRY_LAST_GOOD_META_FILE: &str = "registry-last-good.meta.json";

/// When the cached registry was read, and which document it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryCacheMeta {
    /// RFC3339 instant the authority answered with this document.
    pub read_at: String,
    /// The store's generation/ETag for that document.
    pub generation: String,
}

/// `~/.stado/cache`, or `None` when `HOME` is unset — a daemon started with
/// no environment still reads the registry, it just gets no cache rather
/// than a cache in whatever directory it happened to start in.
fn registry_cache_dir() -> Option<PathBuf> {
    Some(
        Path::new(&std::env::var_os("HOME")?)
            .join(".stado")
            .join("cache"),
    )
}

/// Path of the last-known-good registry document.
pub fn registry_last_good_path() -> Option<PathBuf> {
    Some(registry_cache_dir()?.join(REGISTRY_LAST_GOOD_FILE))
}

/// Path of the sidecar that dates the last-known-good document.
pub fn registry_last_good_meta_path() -> Option<PathBuf> {
    Some(registry_cache_dir()?.join(REGISTRY_LAST_GOOD_META_FILE))
}

/// Write `content` through a per-process temp file and one rename, so a
/// crash leaves the previous file rather than half of the next one.
pub(crate) fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let mut temp = path.to_path_buf();
    // Per-process, because two stado invocations refreshing the cache in the
    // same second must not interleave their bytes in one temp file. The
    // rename can still be lost by the other writer — both wrote the same
    // document, so losing it costs nothing.
    temp.set_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temp, content)?;
    match std::fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// How many hosts a registry document declares.
pub(crate) fn declared_target_count(document: &Value) -> usize {
    document
        .get("targets")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// Whether an incoming document may replace the recorded one.
///
/// A registry with no targets is schema-valid: a fresh install legitimately
/// has none, so the contract cannot refuse it outright. It is still never an
/// improvement on a copy that names hosts. On 2026-08-31 the authority served
/// `{"schema_version":2,"coordinators":[],"targets":[]}` for about nine
/// minutes; it passed the contract, replaced a copy naming three hosts, and
/// every host-addressed command answered `target 'charless-mac-mini' is not in
/// the canonical registry` - from the fallback that exists to survive exactly
/// that outage.
pub(crate) fn may_replace_last_good(
    incoming: &Value,
    recorded: Option<&Value>,
) -> Result<(), String> {
    let arriving = declared_target_count(incoming);
    let held = recorded.map_or(0, declared_target_count);
    if arriving == 0 && held > 0 {
        return Err(format!(
            "the authority served a registry naming no hosts and the recorded copy names {held}; \
             keeping the recorded copy, because an empty fleet is what an outage looks like from \
             here and the fallback exists for outages"
        ));
    }
    Ok(())
}

/// Why the last-known-good copy was not refreshed.
///
/// The cache refused documents silently until 2026-09-03: every refusal
/// printed one stderr line and [`store_last_good`] returned `()`, so neither
/// caller could know the host had stopped taking new copies. A host can sit a
/// registry generation behind indefinitely that way, with the only evidence
/// on a stream nobody keeps.
///
/// The variants are separate because the operator's next step is: fix the
/// document the authority is serving, upgrade this build, look at what the
/// authority just published, or look at the disk. "Not recorded" answers none
/// of those.
///
/// None of this changes WHEN the copy is written. Every document refused
/// before is refused now, for the same reason, and the older copy on disk is
/// still left exactly as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LastGoodRefusal {
    /// This process has no `HOME`, so there is no cache location to write to
    /// and no path to name in a diagnostic either. The only refusal that
    /// prints nothing, because the sentence every other one prints is built
    /// around the document's path.
    NoCacheLocation,
    /// The authority served something that is not JSON. The document is
    /// broken and no build would take it.
    Unparseable { detail: String },
    /// Well-formed, and refused anyway: this build does not accept what the
    /// document declares. The registry is not the fault — the age of this
    /// binary is. Same class as the janitor's `policy:NotImplementedError`.
    RejectedByThisBuild { detail: String },
    /// Valid, and still not an improvement: it names no hosts while the
    /// recorded copy names some. A deliberate safeguard
    /// ([`may_replace_last_good`]) rather than a fault — an empty fleet is
    /// what an outage looks like from here — and it must be visible for that
    /// exact reason: the authority is publishing something the fallback will
    /// not take.
    WouldLoseRecordedHosts { held: usize, detail: String },
    /// Nothing judged the document; the filesystem refused the write.
    NotWritten { detail: String },
}

impl LastGoodRefusal {
    /// A stable slug for a log line, a published state file, or a check.
    /// Bounded and free of paths, values and credentials, so it may be
    /// recorded anywhere the detail sentence may not.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NoCacheLocation => "no-cache-location",
            Self::Unparseable { .. } => "unparseable-document",
            Self::RejectedByThisBuild { .. } => "rejected-by-this-build",
            Self::WouldLoseRecordedHosts { .. } => "would-lose-recorded-hosts",
            Self::NotWritten { .. } => "not-written",
        }
    }

    /// The underlying sentence, which names paths and declared values and so
    /// belongs on stderr and in an operator's terminal, not in the slug.
    pub fn detail(&self) -> &str {
        match self {
            Self::NoCacheLocation => "no HOME, so this process has no registry cache location",
            Self::Unparseable { detail }
            | Self::RejectedByThisBuild { detail }
            | Self::NotWritten { detail } => detail,
            Self::WouldLoseRecordedHosts { detail, .. } => detail,
        }
    }
}

impl std::fmt::Display for LastGoodRefusal {
    /// The historical text. The stderr line this crate has always printed is
    /// built from it verbatim, so an operator's existing grep still matches.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail())
    }
}

/// The last refusal this process saw, for a surface that reads the cache
/// later and has to say why what it is reading is old.
///
/// A refusal may NOT be written down: the whole point is that the cache is
/// left untouched. So the record is process-local, and its reader is
/// [`fetch_registry_or_last_good_detail`], which names it in the notice it
/// already puts in front of an operator when a copy is being served instead
/// of the authority.
static LAST_GOOD_REFUSAL: Mutex<Option<LastGoodRefusal>> = Mutex::new(None);

/// Record a refusal for this process.
pub(crate) fn note_last_good_refusal(refusal: &LastGoodRefusal) {
    *LAST_GOOD_REFUSAL
        .lock()
        .expect("last-known-good refusal lock") = Some(refusal.clone());
}

/// The last refusal this process saw, if any.
pub fn last_good_refusal() -> Option<LastGoodRefusal> {
    LAST_GOOD_REFUSAL
        .lock()
        .expect("last-known-good refusal lock")
        .clone()
}
