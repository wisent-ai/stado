//! Real beacon publication: the document a host publishes about itself, sent
//! through the product's own host-health route and read back through the
//! product's own reader.
//!
//! Nothing here stands anything in. The machine under test is the machine
//! running the test — an isolated registry names this machine's own kernel
//! hostname, so the publisher recognises the document as being about itself
//! and collects its `link` block from this machine's real tools. The route it
//! publishes through is the real Stado dashboard, and the bearer that route
//! compares against is issued by a real Skarbiec vault built from that
//! product's `origin/main`. There is no canned HTTP answer, no substituted
//! executable on `PATH`, and no remote destination.
//!
//! Everything the cases assert is state the product left: the object the
//! listener wrote into the fleet store, the report the reader answers from
//! it, and the exit codes. Every refusal sentence was copied from a live run.

mod broker;
mod cases;
mod fleet;
mod listeners;
