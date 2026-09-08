//! The manifest member records: storage locations, producing-run
//! provenance, the stored verification stamp and the adapter run report.

mod location;
mod producer;
mod report;
mod verification;

pub use location::ArtifactLocation;
pub use producer::ArtifactProducer;
pub use report::VerificationReport;
pub use verification::ArtifactVerification;
