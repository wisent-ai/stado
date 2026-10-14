//! The bulk fan-out this repair shares with `queue::copy`.

/// How many object reads or copies run at once: the parallelism the
/// operating system says this machine has. One shared answer, so no second
/// concurrency number is picked anywhere else.
pub(crate) fn bulk_workers() -> Result<usize, crate::queue::StorageError> {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .map_err(|cause| {
            crate::queue::StorageError::Other(format!(
                "cannot measure available parallelism for bulk object operations: {cause}"
            ))
        })
}
