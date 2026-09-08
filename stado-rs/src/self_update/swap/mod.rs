//! The swap: replace the installed binaries with the verified ones, re-exec
//! this process, and put every OTHER unit that was executing a replaced inode
//! back on the file it declares.

mod launchd;
pub(super) mod recycle;
pub(super) mod replace;
mod systemd;
