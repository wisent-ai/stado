//! What a wait is waiting on: the `rodzaj` field of its lines.

/// What a wait is waiting on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A request to a network service that is not one of the kinds below.
    Network,
    /// A read, write, listing or stat through Stado's object API or another
    /// object store.
    ObjectApi,
    /// A channel to a fleet host: a connection, a remote command, a transfer.
    Host,
    /// A file lock or another lock held by a different process.
    Lock,
    /// A child process this program started and waits for.
    Process,
    /// A database connection or query.
    Database,
    /// A key or a line typed on the terminal the waiting program runs on.
    Terminal,
    /// The operator's answer to a grant a program asked for.
    Consent,
    /// Another agent session's work or answer.
    Session,
}

impl Kind {
    /// Every kind, in declaration order.
    pub const ALL: &'static [Kind] = &[
        Kind::Network,
        Kind::ObjectApi,
        Kind::Host,
        Kind::Lock,
        Kind::Process,
        Kind::Database,
        Kind::Terminal,
        Kind::Consent,
        Kind::Session,
    ];

    /// The word the `rodzaj` field carries.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Network => "siec",
            Kind::ObjectApi => "object-api",
            Kind::Host => "host",
            Kind::Lock => "blokada",
            Kind::Process => "proces",
            Kind::Database => "baza",
            Kind::Terminal => "terminal",
            Kind::Consent => "zgoda",
            Kind::Session => "sesja",
        }
    }

    /// The kind whose `rodzaj` word is `word`.
    pub fn from_word(word: &str) -> Option<Kind> {
        Kind::ALL.iter().copied().find(|kind| kind.word() == word)
    }
}
