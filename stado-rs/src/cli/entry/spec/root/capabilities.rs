//! What `stado capabilities` does beside printing its catalog: the verbs a
//! product uses on a family of alternatives, and the attestation
//! alternative's requests and statements.

use clap::Subcommand;

/// What a product does with a family of alternatives: list them, ask which
/// one answers, hand it a request, choose the order they are tried in, or
/// answer a request in place of a product.
#[derive(Subcommand)]
pub(crate) enum CapabilityAction {
    /// The catalog `stado capabilities` prints: every capability family, its
    /// variants and providers, and each family's order, proofs and what
    /// served whom; FAMILY restricts it to one family.
    List {
        /// Restrict output to one capability family.
        capability: Option<String>,
        /// Emit the versioned machine-readable catalog.
        #[arg(long)]
        json: bool,
    },
    /// The first alternative of FAMILY, in the selected order, proven to
    /// answer CONSUMER, with every one before it that did not; exits with the
    /// infra_down code and names each alternative's product, route and cause
    /// when none answers. A proof stands for the refresh interval this host's
    /// resolver declares; `--failed` proves the named alternative again now.
    Resolve {
        /// The family: inference, secrets, browser-automation, mail or transcripts.
        family: String,
        /// The product asking, as the service directory names its consumer.
        #[arg(long)]
        consumer: String,
        /// An alternative that refused the caller after it was handed out;
        /// proven again before it is handed out once more. Repeat for several.
        #[arg(long = "failed")]
        failed: Vec<String>,
        /// Print the answer, or the refusal's details, as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Send the request on standard input to the first alternative of FAMILY
    /// that answers it, and print that answer: for inference an OpenAI
    /// chat-completions request; for image {"plan", "inputs", "output"}, an
    /// Obraz edit plan; for search {"query", "count"}. The caller names no
    /// gateway, key or alternative: an alternative that cannot answer this
    /// request is named with its cause and the next one is asked; exits with
    /// the infra_down code naming every one when none answers.
    Call {
        /// The family: inference, image or search.
        family: String,
        /// The product asking, as the service directory names its consumer.
        #[arg(long)]
        consumer: String,
        /// Print the resolution beside the answer, and a refusal, as JSON.
        #[arg(long)]
        json: bool,
        /// When the attestation alternative is reached and no statement
        /// answers this request yet, wait for one (`attestation record`)
        /// and print it, instead of refusing and keeping the request.
        #[arg(long)]
        await_statement: bool,
    },
    /// Statements that answer one exact request in place of a product, for
    /// the attestation alternative: the requests `call` kept, read, answered
    /// and withdrawn.
    #[command(subcommand)]
    Attestation(AttestationAction),
    /// Write FAMILY's order to `capabilities.<family>.order` in the config
    /// file; each name must be one of the family's alternatives, once.
    Select {
        /// The family whose order is chosen.
        family: String,
        /// The alternatives in the order they are tried, comma-separated or repeated.
        #[arg(long, value_delimiter = ',', required = true)]
        order: Vec<String>,
    },
}

/// The attestation alternative's requests and statements.
#[derive(Subcommand)]
pub(crate) enum AttestationAction {
    /// Every request of CONSUMER kept for FAMILY, and whether a statement
    /// answers it.
    List {
        family: String,
        #[arg(long)]
        consumer: String,
        #[arg(long)]
        json: bool,
    },
    /// The request as it was asked, and its statement when one is recorded.
    Show {
        family: String,
        #[arg(long)]
        consumer: String,
        /// The request's SHA-256, or the first characters of exactly one.
        request: String,
    },
    /// Record the answer on standard input as the statement answering the
    /// request. For inference: for a request whose answer calls a function,
    /// the JSON object of its arguments; else the answer's text. For image
    /// and search: the answer as the family's alternative gives it.
    Record {
        family: String,
        #[arg(long)]
        consumer: String,
        /// The request's SHA-256, or the first characters of exactly one.
        request: String,
        /// What was checked to give this answer; recorded beside it.
        #[arg(long)]
        checked: String,
        /// The function the answer calls, among those the request offers.
        /// Without it: the one `tool_choice` names, else the only one
        /// offered; a request offering several and naming none is refused.
        #[arg(long)]
        function: Option<String>,
    },
    /// Withdraw the statement answering the request; the request is kept.
    Revoke {
        family: String,
        #[arg(long)]
        consumer: String,
        /// The request's SHA-256, or the first characters of exactly one.
        request: String,
    },
}

