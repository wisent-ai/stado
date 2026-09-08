//! Real current-host journeys for guarded build, attached execution, signal
//! forwarding and recursive run cleanup.
//!
//! Every case declares this machine as its registry target — the kernel's own
//! hostname, normalized — and gives it an isolated `HOME`, so the product's
//! current-host branch runs the real compiler and the real programs here.
//! There is no remote destination in any fixture and no executable
//! substituted on `PATH`: the only thing the fixture puts in front of the
//! product is a symlink to the operating system's own Cargo.
//!
//! Every success case reads the state the process itself left: the file the
//! program wrote, the binary the real Cargo produced, the exit code the
//! program's own trap chose, and the directory the removal verb deleted.
//! Every refusal sentence was copied from a live run, and each refusal case
//! also proves that nothing ran and nothing was removed.

mod execute;
mod fixture;
mod refusals;
