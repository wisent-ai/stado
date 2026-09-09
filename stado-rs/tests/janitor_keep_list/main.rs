//! The janitor's workdir keep-list, as the trees it leaves on this machine.
//!
//! # What happened
//!
//! `disk_cleanup` builds a keep-list of every job id in `queue/` and
//! `running/` before it deletes a job workdir, and it used
//! `JobStorage::list_jobs(prefix, 0)` to do it — which lists the prefix and
//! then DOWNLOADS every job document, ten at a time, to read `job_id` out of a
//! body whose object name already carried it. On 2026-09-03 charless-mac-mini
//! published `duration_ms: 818021` — 13.6 minutes — for a pass whose own
//! verdict was `healthy_noop`, and not one of those downloads was ever read.
//!
//! # What is defended here, and where an operator sees it
//!
//! The keep-list is not a number an operator can read. What they can read is
//! which job trees survived a pass, so that is what every case here asserts,
//! after running `stado disk-cleanup` against a store this machine really
//! holds:
//!
//! - a job the store still calls `running` keeps its tree, and a job with a
//!   terminal record loses it — the whole gate, on a population the product
//!   itself created through `stado submit` and a real local agent;
//! - a job whose document cannot be parsed at all keeps its tree, which is the
//!   cost model made visible: ids come from object NAMES, and a keep-list that
//!   read bodies would drop that job and delete the tree under it;
//! - a queue prefix that cannot be read reclaims NOTHING, because an
//!   unavailable authority is not permission to delete.
//!
//! # What this area used to do, and no longer does
//!
//! It wrapped `LocalBackend` in a counting stub, called `live_job_ids_within`
//! directly and asserted the returned `Vec<String>`. No command ran, no
//! workdir existed, and nothing on disk was ever kept or removed.
//!
//! DELETED, with the reason.
//!
//! `the_keep_list_downloads_no_job_documents` and
//! `a_job_mid_transition_keeps_its_place_on_the_list` measured the same
//! property through a stub's download counter and a returned vector. The
//! property is real and it is now
//! `cases::a_job_whose_document_cannot_be_read_keeps_its_workdir`: a job
//! carrying bytes no reader can parse still keeps its tree, which is only true
//! if the id came from the name.
//!
//! `the_priority_index_is_not_mistaken_for_the_queue` asserted that
//! `list_job_ids("queue")` does not collect `queue_priority/`. The guard is a
//! `strip_prefix` at the delimiter inside the listing, and on every backend
//! this machine can run — the local filesystem store — `queue_priority/` is a
//! different directory that no `queue/` listing returns in the first place. No
//! command, no report field and no surviving or deleted tree changes with it,
//! so the case could only ever have asserted the return value of a private
//! listing helper.
//!
//! `a_stalled_store_expires_inside_its_budget` asserted that
//! `live_job_ids_within` gives up on its budget rather than on the transport,
//! by handing it a backend that sleeps ten minutes. Nothing this machine can
//! put behind `WC_STORAGE_BACKEND=local` stalls, so the timeout itself has no
//! CLI surface. What that bound exists to protect — a pass that cannot read
//! the authority must delete nothing — is
//! `cases::an_unreadable_queue_prefix_reclaims_nothing`, which takes the
//! authority away for real.

mod cases;
mod fixture;
