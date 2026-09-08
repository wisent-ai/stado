//! Declaration surfaces and the consumer that honours what an operator
//! writes on them.

use serde::Serialize;

// ---------------------------------------------------------------------------
// Declaration catalog: which reader honours a field an operator writes
// ---------------------------------------------------------------------------

/// Which document carries a declaration, and therefore which reader would have
/// to consult it for the declaration to mean anything.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeclarationSurface {
    /// A key on one `registry.targets[]` entry, dotted from the target root
    /// (`weles.actions`).
    RegistryTarget,
    /// A key in the deployed configuration document, dotted from its root
    /// (`storage.stado.ca_file`).
    Configuration,
}

impl DeclarationSurface {
    pub const fn label(self) -> &'static str {
        match self {
            Self::RegistryTarget => "registry target",
            Self::Configuration => "config",
        }
    }
}

/// What turns a declaration into behaviour. Writing a field is free; being read
/// is the whole value of having written it, and the two have to be recorded
/// together or nobody can tell them apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Consumer {
    /// A fleet process reads the declaration and acts on it. The string names
    /// the exact code path, so the claim is checkable rather than asserted.
    Fleet(&'static str),
    /// Nothing in this build reads it at all.
    Unread,
}

/// A sibling declaration that decides whether a reader can act on this one.
///
/// `storage.stado.ca_file` is the case in hand: the certificate is loaded only
/// on the TLS path, so on a deployment whose `storage.stado.url` is plain
/// loopback HTTP the reader exists and still never consults the value. A field
/// whose reader cannot be reached is as unread as one with no reader.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct SiblingCondition {
    /// Dotted path of the sibling, from the same surface root.
    pub path: &'static str,
    /// Prefix the sibling's value must carry for the reader to run.
    pub value_prefix: &'static str,
}

/// One declaration an operator can write, and the reader that honours it.
///
/// This is [`ConfigField`]'s idea — a key reaches this binary only through a
/// catalogued entry — carried across to the fields a registry target declares,
/// where nothing enforced it: `weles.actions` sat on two hosts with no fleet
/// reader at all, and `storage.stado.ca_file` sat in the deployed configuration
/// for months while every validator compared the document against itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeclaredField {
    /// Dotted path from the surface's root.
    pub path: &'static str,
    pub surface: DeclarationSurface,
    pub consumer: Consumer,
    /// Condition the surrounding document must satisfy for the consumer above
    /// to be reachable at all.
    pub reached_when: Option<SiblingCondition>,
}

impl DeclaredField {
    pub const fn read(
        path: &'static str,
        surface: DeclarationSurface,
        reader: &'static str,
    ) -> Self {
        Self {
            path,
            surface,
            consumer: Consumer::Fleet(reader),
            reached_when: None,
        }
    }

    pub const fn unread(path: &'static str, surface: DeclarationSurface) -> Self {
        Self {
            consumer: Consumer::Unread,
            ..Self::read(path, surface, "")
        }
    }

    pub const fn reached_when(mut self, path: &'static str, value_prefix: &'static str) -> Self {
        self.reached_when = Some(SiblingCondition { path, value_prefix });
        self
    }

    /// True when a declared value reaches a process that acts on it without an
    /// operator retyping a command first.
    pub const fn has_fleet_consumer(&self) -> bool {
        matches!(self.consumer, Consumer::Fleet(_))
    }
}

/// Every declaration whose reader (or absence of one) this build can name.
///
/// The catalog is authoritative for the paths it lists. It deliberately does NOT
/// list every modelled registry key: a key `ComputeTarget` models is read by the
/// deserializer and the code around it, and a key it does not model lands in
/// `ComputeTarget::extra`, which is by construction the set of keys no typed
/// reader in this build can name. `registry doctor` reports an unmodelled key
/// that is absent from here, so a field added tomorrow with no reader fails
/// without anybody remembering to catalogue it — the entries below are the
/// exceptions, not the mechanism.
pub const DECLARED_FIELDS: &[DeclaredField] = &[
    // Unmodelled target keys that genuinely are read, out of raw JSON.
    DeclaredField::read(
        "services",
        DeclarationSurface::RegistryTarget,
        "cli::registry::declared_units, deploy::service, cli::service",
    ),
    DeclaredField::read(
        "service_resolver",
        DeclarationSurface::RegistryTarget,
        "service_resolution::resolver_config",
    ),
    DeclaredField::read(
        "gpu_power_limit_watts",
        DeclarationSurface::RegistryTarget,
        "providers::local::agent::reconcile_gpu_power_limit",
    ),
    // The host's own refusal, next to what it can do: an excluded capability is a
    // policy answer ("may not run here") and not a measurement, so the matcher
    // reports it as a distinct reason rather than as a host that failed a probe.
    // The reader is the placement matcher, which is Python today; `Fleet` asks for
    // the reader's name, not for its language.
    DeclaredField::read(
        "placement",
        DeclarationSurface::RegistryTarget,
        "scripts/place-by-capability.py",
    ),
    // Label-model training placement. The reader is another repository's binary
    // -- `transcript-label-trainer`, `placement::declared_training` and
    // `placement::declared_lake_root` -- which is as much a reader as a Python
    // script is: `Fleet` asks for the reader's name, not for its language or its
    // checkout. Uncatalogued, both keys read as unread while a trainer honoured
    // them, and `training.models_dir` sat pointing at a disk that had been
    // removed from the host.
    DeclaredField::read(
        "training",
        DeclarationSurface::RegistryTarget,
        "transcript-label-trainer placement::declared_training",
    ),
    DeclaredField::read(
        "transcript_lake",
        DeclarationSurface::RegistryTarget,
        "transcript-label-trainer placement::declared_lake_root",
    ),
    // `weles.actions` is modelled (`WelesPolicy::actions`) and the worker reads
    // its own `placement-policy.json`, never the registry. For a long time the
    // only thing joining the two was an operator running `stado host
    // publish-placement-policy`, so they drifted: the registry listed
    // `apple_create_developer_id` and the host file did not, and the worker
    // declined the row in silence for hours while the declaration said it was
    // allowed. The host's own agent now writes that file from this declaration
    // on every pass, which is what makes the value reach behaviour without
    // anybody retyping a command.
    DeclaredField::read(
        "weles.actions",
        DeclarationSurface::RegistryTarget,
        "providers::local::agent::reconcile_placement_policy",
    ),
    DeclaredField::read(
        "storage.stado.ca_file",
        DeclarationSurface::Configuration,
        "queue::stado_object::StadoObjectBackend::client",
    )
    .reached_when("storage.stado.url", "https://"),
];

/// The catalogued declaration for one dotted path on one surface.
pub fn declared_field(surface: DeclarationSurface, path: &str) -> Option<&'static DeclaredField> {
    DECLARED_FIELDS
        .iter()
        .find(|field| field.surface == surface && field.path == path)
}
