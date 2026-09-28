//! One verified copy-on-write clone of a file into place. The clone is
//! staged under the transaction's own directory, named by the destination,
//! so an interrupted clone is found and either reused or replaced on resume.

use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use sha2::{Digest, Sha256};

use super::darwin;
use super::{
    digest, fsync_dir, join, make_private_dirs, parent_of, privileged_clone,
    recover_privileged_clone, regular_identity, PRIVATE_FILE,
};
use crate::deploy::host_storage_reconcile_program::{Context, Step};

pub(in crate::deploy::host_storage_reconcile_program) fn clone_file(
    context: &Context,
    source: &str,
    destination: &str,
) -> Step<()> {
    make_private_dirs(&parent_of(destination))?;
    if let Ok(info) = fs::symlink_metadata(&context.staging) {
        if !info.is_dir() {
            return Err("unsafe transaction clone staging root".to_string());
        }
    }
    make_private_dirs(&context.staging)?;
    let temporary = join(
        &context.staging,
        &hex::encode(Sha256::digest(destination.as_bytes())),
    );
    if let Ok(info) = fs::symlink_metadata(&temporary) {
        let staged = regular_identity(context, &temporary)?;
        if info.uid() != nix::unistd::getuid().as_raw()
            || darwin::flags(&info) & darwin::ANY_IMMUTABLE != 0
        {
            recover_privileged_clone(context, &temporary)?;
        }
        if staged != regular_identity(context, source)? {
            fs::remove_file(&temporary)
                .map_err(|error| format!("cannot remove stale clone {temporary}: {error}"))?;
        }
    }
    if fs::metadata(&temporary).is_err() {
        if let Err(error) = darwin::clone(source, &temporary) {
            if !matches!(
                error.raw_os_error(),
                Some(nix::libc::EACCES) | Some(nix::libc::EPERM)
            ) {
                return Err(format!("clonefile refused copy-on-write clone: {error}"));
            }
            if fs::symlink_metadata(&temporary).is_ok() {
                return Err("clonefile left a partial privileged clone destination".to_string());
            }
            privileged_clone(context, source, &temporary)?;
        }
    }
    if digest(context, source)? != digest(context, &temporary)? {
        return Err("copy-on-write clone verification failed".to_string());
    }
    let settled = (|| -> io::Result<()> {
        if darwin::SUPPORTED {
            darwin::set_flags(&temporary, 0)?;
        }
        fs::set_permissions(&temporary, fs::Permissions::from_mode(PRIVATE_FILE))?;
        fs::File::open(&temporary)?.sync_all()?;
        fs::rename(&temporary, destination)
    })();
    settled.map_err(|error| format!("cannot place clone at {destination}: {error}"))?;
    fsync_dir(&parent_of(destination))
}
