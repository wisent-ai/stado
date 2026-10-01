//! File permission bits, built from the platform's own `S_I*` constants so no
//! mode is spelled as a number.

use nix::sys::stat::Mode;

/// Read and write for the owner and nothing for anyone else: the mode every
/// secret, receipt and key file is created with.
pub fn owner_read_write() -> u32 {
    u32::from((Mode::S_IRUSR | Mode::S_IWUSR).bits())
}

/// Whether a file mode grants anything to the group or to other users.
pub fn open_to_others(mode: u32) -> bool {
    Mode::from_bits_truncate(mode as nix::libc::mode_t).intersects(Mode::S_IRWXG | Mode::S_IRWXO)
}
