//! What a process holds or waits on in the machine's wait-for graph: a lock
//! file, a host, a service, a product revision, a unit or a port. Every
//! resource is written in the graph's files as `<kind>:<id>`.

use std::fmt;
use std::path::PathBuf;

/// What sort of thing a resource is: the `<kind>` of `<kind>:<id>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceKind {
    /// A lock file; its holder is also the `"pid"` its own file names.
    Lock,
    /// A fleet host a process holds (a channel it keeps) or waits to reach.
    Host,
    /// A service by name, held by the process that serves it.
    Service,
    /// A product revision (`<product>@<revision>`), held by the operation
    /// that delivers it.
    ProductRevision,
    /// A service-manager unit, held by the process that manages it.
    Unit,
    /// A listening port (`<address>:<port>`), held by the process bound to it.
    Port,
}

impl ResourceKind {
    /// Every kind, in declaration order.
    pub const ALL: &'static [ResourceKind] = &[
        ResourceKind::Lock,
        ResourceKind::Host,
        ResourceKind::Service,
        ResourceKind::ProductRevision,
        ResourceKind::Unit,
        ResourceKind::Port,
    ];

    /// The `<kind>` word of `<kind>:<id>`.
    pub fn word(self) -> &'static str {
        match self {
            ResourceKind::Lock => "lock",
            ResourceKind::Host => "host",
            ResourceKind::Service => "service",
            ResourceKind::ProductRevision => "revision",
            ResourceKind::Unit => "unit",
            ResourceKind::Port => "port",
        }
    }

    /// The words of every kind, for a refusal that lists them.
    pub fn words() -> String {
        let words: Vec<&str> = Self::ALL.iter().map(|kind| kind.word()).collect();
        words.join(", ")
    }
}

/// One thing a process can hold or wait for: its kind and its id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Resource {
    pub kind: ResourceKind,
    pub id: String,
}

impl Resource {
    /// The lock file `path`.
    pub fn lock(path: &std::path::Path) -> Self {
        Self {
            kind: ResourceKind::Lock,
            id: path.display().to_string(),
        }
    }

    /// The resource of `kind` named `id`. An empty id or one holding a
    /// control character is the refusal naming it.
    pub fn new(kind: ResourceKind, id: impl Into<String>) -> Result<Self, String> {
        let id = id.into();
        if id.trim().is_empty() || id.chars().any(char::is_control) {
            return Err(format!(
                "a {} resource needs an id that is not empty and holds no control character; got {id:?}",
                kind.word()
            ));
        }
        Ok(Self { kind, id })
    }

    /// The resource `<kind>:<id>` names.
    pub fn parse(text: &str) -> Result<Self, String> {
        let Some((word, id)) = text.split_once(':') else {
            return Err(format!(
                "{text:?} is no resource: a resource is <kind>:<id>, kind one of {}",
                ResourceKind::words()
            ));
        };
        let Some(kind) = ResourceKind::ALL
            .iter()
            .copied()
            .find(|kind| kind.word() == word)
        else {
            return Err(format!(
                "{word} is no resource kind; a resource is <kind>:<id>, kind one of {}",
                ResourceKind::words()
            ));
        };
        Self::new(kind, id)
    }

    /// What a graph file holds: `<kind>:<id>`. A file written before the
    /// graph held more than locks holds a bare absolute lock path, which is
    /// read as that lock.
    pub(crate) fn read(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.starts_with(std::path::MAIN_SEPARATOR) {
            return Ok(Self::lock(std::path::Path::new(text)));
        }
        Self::parse(text)
    }

    /// The `<kind>:<id>` the graph's files hold.
    pub fn key(&self) -> String {
        format!("{}:{}", self.kind.word(), self.id)
    }

    /// The lock file this resource is, when it is one.
    pub fn lock_path(&self) -> Option<PathBuf> {
        (self.kind == ResourceKind::Lock).then(|| PathBuf::from(&self.id))
    }
}

impl fmt::Display for Resource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.kind.word(), self.id)
    }
}
