//! Archives whose bytes depend only on what they hold.
//!
//! Every archive Stado builds is compared or published by its digest — a
//! release payload, a build's staged output, a web artifact, a storage
//! transfer — so the same tree must give the same bytes on every builder and
//! every run. What a tar or gzip header records besides the content, when, by
//! whom and under which umask, is fixed here once: the Unix epoch, the root
//! user, and the two modes `git archive` writes, as
//! <https://reproducible-builds.org/docs/archives/> prescribes for an archive
//! whose members have no meaningful origin.

use nix::sys::stat::Mode;
use nix::unistd::ROOT;

use crate::primitives::file_mode;

/// The modification time every member carries: the Unix epoch itself.
pub fn epoch_seconds() -> u64 {
    std::time::Duration::ZERO.as_secs()
}

/// The gzip header's modification time: the epoch, which gzip also reads as
/// "no time stated" (RFC 1952, section 2.3.1). The epoch is the zero of every
/// width, so narrowing it to gzip's 32-bit field loses nothing.
pub fn gzip_epoch() -> u32 {
    epoch_seconds() as u32
}

/// A file's mode as an archive records it: nothing of the builder's umask
/// survives, only whether its owner may run it — [`file_mode::program`] if
/// so, [`file_mode::shared_read`] otherwise.
pub fn member_mode(mode: u32) -> u32 {
    if Mode::from_bits_truncate(mode as nix::libc::mode_t).contains(Mode::S_IXUSR) {
        file_mode::program()
    } else {
        file_mode::shared_read()
    }
}

/// The mode every directory member carries: searchable by everyone, like a
/// program.
pub fn directory_mode() -> u32 {
    file_mode::program()
}

/// The mode every symlink member carries. No extractor applies a symlink's
/// mode, and lstat reports a different one on Darwin than on Linux, so it is
/// fixed rather than copied: every access bit, as Linux reports a link.
pub fn symlink_mode() -> u32 {
    file_mode::access_bits()
}

/// Fix one member header's origin: the epoch as its time, the root user as
/// its owner and group (the root group shares the root user's id), and only
/// the access bits of `mode`. A GNU header starts with no owner or group
/// name, so none is recorded.
pub fn stamp(header: &mut tar::Header, mode: u32) {
    let root = u64::from(ROOT.as_raw());
    header.set_mtime(epoch_seconds());
    header.set_uid(root);
    header.set_gid(root);
    header.set_mode(mode & file_mode::access_bits());
}
