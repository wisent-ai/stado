//! File permission bits, built from the platform's own `S_I*` constants so no
//! mode is spelled as a number.

use nix::sys::stat::Mode;

/// Read and write for the owner and nothing for anyone else: the mode every
/// secret, receipt and key file is created with.
pub fn owner_read_write() -> u32 {
    widen((Mode::S_IRUSR | Mode::S_IWUSR).bits())
}

/// Read, write and search for the owner and nothing for anyone else: the
/// mode of a directory that holds such files.
pub fn owner_only_directory() -> u32 {
    widen(Mode::S_IRWXU.bits())
}

/// Read, write and run for the owner and nothing for anyone else: the mode
/// of a program only its owner runs, such as a transaction tool staged under
/// the owner's home for the host's own process to execute.
pub fn owner_program() -> u32 {
    widen(Mode::S_IRWXU.bits())
}

/// Read, write and run for the owner, read and run for everyone else: the
/// mode of a program other users and services execute.
pub fn program() -> u32 {
    widen((Mode::S_IRWXU | Mode::S_IRGRP | Mode::S_IXGRP | Mode::S_IROTH | Mode::S_IXOTH).bits())
}

/// Read and write for the owner, read for everyone else: the mode of a
/// regular file as `git archive` writes it, and of a record added beside one.
pub fn shared_read() -> u32 {
    widen((Mode::S_IRUSR | Mode::S_IWUSR | Mode::S_IRGRP | Mode::S_IROTH).bits())
}

/// Read for the owner and nothing for anyone else: a file sealed so nothing
/// can change it.
pub fn owner_read_only() -> u32 {
    widen(Mode::S_IRUSR.bits())
}

/// Read and search for the owner and nothing for anyone else: a directory
/// sealed so nothing can be added to it or removed from it.
pub fn owner_read_search() -> u32 {
    widen((Mode::S_IRUSR | Mode::S_IXUSR).bits())
}

/// Every bit `chmod` sets and `stat.S_IMODE` keeps: set-user-ID, set-group-ID,
/// sticky and the nine access bits, without the file-type bits.
pub fn permission_bits() -> u32 {
    widen(
        (Mode::S_ISUID
            | Mode::S_ISGID
            | Mode::S_ISVTX
            | Mode::S_IRWXU
            | Mode::S_IRWXG
            | Mode::S_IRWXO)
            .bits(),
    )
}

/// The nine access bits alone, for owner, group and others: what an archive
/// member keeps of a file's mode, without set-ID or sticky bits.
pub fn access_bits() -> u32 {
    widen((Mode::S_IRWXU | Mode::S_IRWXG | Mode::S_IRWXO).bits())
}

/// Bits libc names for a file's mode (`S_IRUSR`, `S_IFMT`, a `Mode`'s bits),
/// as `std` takes and gives a mode: `mode_t` is already `u32` on Linux and
/// narrower on Darwin, so the widening is a lossless conversion on one
/// platform and the identity on the other; written as a cast or a `From`,
/// it is refused on Linux as a conversion to its own type. Usable in a
/// constant.
#[cfg(target_os = "linux")]
pub const fn widen(mode: nix::libc::mode_t) -> u32 {
    mode
}

#[cfg(not(target_os = "linux"))]
pub const fn widen(mode: nix::libc::mode_t) -> u32 {
    mode as u32
}

/// Whether a file mode grants anything to the group or to other users.
pub fn open_to_others(mode: u32) -> bool {
    Mode::from_bits_truncate(mode as nix::libc::mode_t).intersects(Mode::S_IRWXG | Mode::S_IRWXO)
}

/// Whether a file mode lets its owner, its group or anyone else run it.
pub fn executable_by_anyone(mode: u32) -> bool {
    Mode::from_bits_truncate(mode as nix::libc::mode_t)
        .intersects(Mode::S_IXUSR | Mode::S_IXGRP | Mode::S_IXOTH)
}
