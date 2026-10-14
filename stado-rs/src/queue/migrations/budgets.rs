//! The bulk fan-out this repair shares with `queue::copy`.

/// How many object reads or copies run at once: the parallelism the
/// operating system says this machine has. One shared answer, so no second
/// concurrency number is picked anywhere else.
pub(crate) fn bulk_workers() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}
