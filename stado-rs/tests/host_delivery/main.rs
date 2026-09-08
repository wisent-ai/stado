//! Real current-host journeys for target-scoped run-input delivery.
//!
//! The registry target these cases name is the machine executing them: its
//! only entry carries this machine's own kernel hostname, normalized, so
//! `deploy::host_channel` takes its current-host branch and the delivery runs
//! this machine's real `rsync`, real `/bin/sh` guards and real atomic
//! replacement. There is no destination in the fixture and no executable
//! substituted on `PATH` — `PATH` is the operating system's own directories.
//!
//! Isolation is total: a fresh tempdir per case holds `HOME` and a local
//! storage backend, `STADO_CONFIG` names a path that does not exist, and the
//! delivered bytes land under that tempdir's `$HOME/.stado/work/runs`. The
//! operator's registry, home, fleet store and managed runs are never read and
//! never written.
//!
//! Every assertion reads state the product left behind: the bytes on disk,
//! the mode on the delivered file, the symlink it preserved, and the exit
//! code. Stdout is corroboration only, and every refusal sentence here was
//! copied from a live run.

mod deliver;
mod fleet;
mod refusals;
