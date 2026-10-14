//! The cleaner's registry name and the three fixed names macOS itself uses
//! for the clone container, the per-application clone root and each clone.

/// The cleaner's registry name, and the key its counts appear under in the
/// janitor's report. Declared here rather than spelled at each use, because
/// [`crate::targets`]'s allowed-cleaner list, the report, and this scan have
/// to name the same cleaner or a policy authorizes a pass that never runs.
pub const CLEANER: &str = "chromium_clones";

/// The directory macOS keeps code-sign clones in, beside the `T` (temporary)
/// and `C` (cache) directories of the same per-user container.
///
/// One letter, unhelpfully, and there is no `confstr` variable for it — only
/// for its siblings — so the container is resolved through
/// `darwin_user_temp_dir` and this name is joined onto it.
pub const CLONE_CONTAINER: &str = "X";

/// The per-application clone root. `org.chromium.Chromium` is the bundle
/// identifier of the Chromium builds Weles drives; Chrome, Safari and every
/// other signed app get their own root beside it and are NOT this cleaner's
/// business, because the measurement that authorized this code is Chromium's.
pub const CLONE_ROOT_NAME: &str = "org.chromium.Chromium.code_sign_clone";

/// The prefix macOS gives every clone it makes in that root
/// (`code_sign_clone.XXXXXX`).
///
/// Required of a candidate: an entry the OS did not name is not a clone, and
/// the one thing this cleaner must never do is delete something that merely
/// happens to live in a directory it was pointed at. Exported so
/// [`crate::deploy::host_reclaim`]'s stage requires the same name of the same
/// entries — two spellings would be two definitions of "clone", and the
/// operator would meet whichever ran first.
pub const CLONE_ENTRY_PREFIX: &str = "code_sign_clone.";
