//! What a credential read or write can fail with, and the failure code each
//! failure states, so no caller has to classify a vault failure by its words.

use crate::primitives::failure::FailureCode;

#[derive(Debug, thiserror::Error)]
pub enum SkarbiecError {
    #[error("invalid Skarbiec URL {0:?}; expected loopback HTTP or HTTPS")]
    InvalidUrl(String),
    #[error("Skarbiec consumer is not configured; set WC_SKARBIEC_CONSUMER")]
    MissingConsumer,
    #[error("Skarbiec grant file is not configured; set WC_SKARBIEC_TOKEN_FILE")]
    MissingTokenFile,
    #[error("cannot read Skarbiec grant file {path}: {source}")]
    TokenFile {
        path: String,
        source: std::io::Error,
    },
    #[error("Skarbiec grant file {0} must not be accessible by group or other users")]
    InsecureTokenFile(String),
    #[error("Skarbiec grant file {0} is empty")]
    EmptyToken(String),
    #[error("Skarbiec request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Skarbiec returned HTTP {status}: {detail}")]
    Response { status: u16, detail: String },
    #[error("Skarbiec item {0:?} has no value")]
    MissingValue(String),
    #[error("Skarbiec deployment configuration: {0}")]
    Deployment(String),
    #[error("cannot acquire GCP workload or Skarbiec identity: {0}")]
    GcpAuth(String),
    /// What is stored is an encrypted `{"v":…,"c":…}` envelope, not the
    /// value: handed on, it fails later in the consumer as something else.
    #[error("the stored text is a {version} ciphertext envelope, not the value; store the plain value again with `stado credentials put`")]
    StoredEnvelope { version: String },
    /// A read that failed, carrying the coordinates of the read and the
    /// failure underneath it unchanged.
    ///
    /// Every verifier boundary reads a `token` field through four different
    /// clients — object, release, service and machine — and a refusal in any
    /// of them used to surface as one indistinguishable sentence: `Skarbiec
    /// returned HTTP 403: {"error":"consumer not authorized to read item
    /// field"}`, with no consumer, no item and no field in it, which
    /// `stado doctor --deployment-preflight` then reports under whichever
    /// verifier happened to read, for a read that verifier does not
    /// necessarily own.
    ///
    /// The variant underneath is preserved rather than flattened into a
    /// message, so `is_unavailable`, `status` and every other typed question
    /// keep answering about the real failure.
    #[error("consumer {consumer:?} reading {item:?} field {field:?}: {source}")]
    Read {
        consumer: String,
        item: String,
        field: String,
        #[source]
        source: Box<SkarbiecError>,
    },
}

impl SkarbiecError {
    /// Whether this says the vault could not be REACHED or could not answer,
    /// as opposed to answering that something is configured wrongly.
    ///
    /// The distinction is the difference between a verdict and silence, and
    /// it is typed here rather than recovered from a message downstream: a
    /// classifier that substring-matches an error sentence is the defect this
    /// repository has already paid for twice.
    ///
    /// A 5xx is the vault's own statement that it is unavailable — Skarbiec
    /// answers `503 {"error_code":"infra_down"}` while its GnuPG daemons are
    /// wedged, for items whose keys are present and whose grants are intact.
    /// A transport error never reached an opinion at all. Neither one says
    /// anything about mapping, grants or tokens.
    pub fn is_unavailable(&self) -> bool {
        match self {
            Self::Http(_) => true,
            Self::Response { status, .. } => *status >= 500,
            Self::Read { source, .. } => source.is_unavailable(),
            _ => false,
        }
    }

    /// The HTTP status the vault answered with, if it answered at all.
    ///
    /// Callers used to match `Response { status, .. }` on a read's own result,
    /// which stops matching the moment the read is named. Ask the question
    /// instead of destructuring the shape.
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Response { status, .. } => Some(*status),
            Self::Read { source, .. } => source.status(),
            _ => None,
        }
    }

    /// Whether the vault answered that this item or field is not there, as
    /// opposed to refusing it or failing to answer.
    pub fn is_missing(&self) -> bool {
        match self {
            Self::MissingValue(_) => true,
            Self::Response { status, .. } => *status == reqwest::StatusCode::NOT_FOUND.as_u16(),
            Self::Read { source, .. } => source.is_missing(),
            _ => false,
        }
    }

    /// Name the read this failure came from, once. A failure that already
    /// carries coordinates keeps the innermost ones, so a caller wrapping a
    /// wrapped read cannot bury the item that was actually refused.
    pub(crate) fn naming(self, consumer: &str, item: &str, field: &str) -> Self {
        if matches!(self, Self::Read { .. }) {
            return self;
        }
        let consumer = if consumer.trim().is_empty() {
            "credential-store".to_string()
        } else {
            consumer.trim().to_string()
        };
        Self::Read {
            consumer,
            item: item.to_string(),
            field: field.to_string(),
            source: Box::new(self),
        }
    }

    /// The failure code this failure states, decided by its variant and the
    /// status the vault answered, never by its sentence. Every variant is
    /// named, so a new one cannot reach an operator unclassified.
    pub fn failure_code(&self) -> FailureCode {
        match self {
            Self::Http(_) => FailureCode::InfraDown,
            // A 4xx the upstream table leaves unclassified is still the
            // vault answering that it refuses this request.
            Self::Response { status, .. } => match FailureCode::from_upstream_status(*status) {
                FailureCode::Unknown if (400..500).contains(status) => FailureCode::Refused,
                code => code,
            },
            Self::MissingValue(_) => FailureCode::NotFound,
            Self::GcpAuth(_) => FailureCode::Auth,
            Self::InvalidUrl(_)
            | Self::MissingConsumer
            | Self::MissingTokenFile
            | Self::TokenFile { .. }
            | Self::InsecureTokenFile(_)
            | Self::EmptyToken(_)
            | Self::Deployment(_)
            | Self::StoredEnvelope { .. } => FailureCode::Config,
            Self::Read { source, .. } => source.failure_code(),
        }
    }
}
