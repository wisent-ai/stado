//! What this fleet delivers, declared instead of hardcoded.
//!
//! `stado host release` used to carry a compile-time table of two entries —
//! `stado` and `skarbiec` — and a host asking for anything else was told
//! `"weles-worker" is not a stado-managed binary`. That refusal was wrong
//! about the fleet rather than about the request: the registry already
//! declared `weles-worker 0.5.1` for `control-host` under
//! `targets[].managed_versions`, `stado service converge` already read the
//! installed `0.5.0` off the artefact tree and already called the drift, and
//! the only thing missing was the delivery half. A product the fleet declares
//! and measures but cannot deliver is a drift report nobody can close.
//!
//! So a deliverable product is a DECLARATION, and there is exactly one of
//! them: [`DECLARATION_PATH`], baked into the binary at compile time the same
//! way the bundled registry snapshot and the provider startup templates are
//! ([`crate::targets::load_bundled_registry`],
//! [`crate::scheduler::dispatch::agent::bundled_template_for`]). `stado` and
//! `skarbiec` are two entries in it with no standing the third does not have.
//!
//! Why a shipped document rather than `targets[].managed_versions`'s
//! neighbour in the canonical registry: the two answer different questions.
//! *Which version must this host run* is per-host operator intent, changes
//! without a release, and belongs in the registry — it already lives there.
//! *What is this product, where does it install, which units own it, and how
//! is its installed version read back* is a property of the release that
//! produced the artefact: it changes only when the product's own
//! `.wisent-release.json` changes, it must be identical on every host, and a
//! delivery built from a stale copy of it would install a tree in the wrong
//! place. It ships with the binary that performs the delivery, so the two
//! cannot disagree.
//!
//! What one declaration has to name, and why each field is required rather
//! than defaulted:
//!
//! - **the artefact source** ([`Source`]) — the `stado://releases/<product>/…`
//!   segment and the exact archive member to take out of it. Defaulting the
//!   member to the product name is how `weles-worker` would have silently
//!   looked for a file called `weles-worker` inside an archive that carries
//!   `payload/weles-worker.tar.gz`, and reported "layout" instead of naming
//!   the mistake.
//! - **the platform keys** (`platforms`) — the published coordinate segments,
//!   a subset of [`PLATFORMS`]. `stado` publishes both, `weles-worker` only
//!   `darwin-arm64`; a delivery to a host on an unpublished platform is
//!   refused on the control plane instead of fetching a 404.
//! - **the install root on the host** ([`Install`]) — `$HOME/.stado/bin` for a
//!   program, the artefact directory itself for a tree. A tree also declares
//!   the host-local paths a delivery must leave alone (`preserve`), because
//!   `$HOME/weles` holds `recordings/`, `var/` and `.work/` that no release
//!   produced and no release may take away.
//! - **the owning units** ([`Unit`]) — Stado's resolver, coordinator, queue
//!   agent and release agent declare the binary they execute. Root-installed
//!   owners activate here; independently installed service-tree owners consume
//!   the same verified archive through reader convergence before their restart.
//! - **how the installed version is read back** ([`Readback`]) — running the
//!   program for a program, one member of one JSON file inside the tree for a
//!   tree. This is the field that decides whether a host is already at the
//!   requested version, so a product that cannot be read back is a product
//!   whose delivery could never be checked.
//!
//! Only lists are optional-with-a-default: no units and no superseded roots
//! are both real declarations. Every other missing field is a refusal naming
//! the field, and [`validate`] refuses the
//! declarations serde cannot: an unknown platform, a root outside `$HOME`, a
//! `..` in a member, a preserved path the artefact would overwrite, a version
//! readback that does not match what was installed, two products with one
//! name. The refusals are made once, when the declaration is first read, and
//! cached — a malformed document fails every delivery identically instead of
//! failing the ones whose code path happens to look.

mod declaration;
mod reading;
mod validation;

pub use declaration::{Declaration, Install, Product, Readback, Shape, Source, Unit};
pub use reading::{declared, installed_programs, managed_platform, product};
pub use validation::validate;

/// The one file that says what this fleet can deliver, named in refusals so
/// an operator adding a product knows the single place to write it.
pub const DECLARATION_PATH: &str = "stado-rs/data/catalog/products.json";

/// The declaration itself, read at compile time. Reading it back through
/// [`crate::data_dir`] at runtime only ever worked on the build machine.
const DECLARATION: &str = include_str!("../../../data/catalog/products.json");

/// The declaration schema this build understands. A document from the future
/// is refused rather than partially honoured.
pub const SCHEMA_VERSION: u64 = 2;

/// The platform coordinate segments this fleet publishes for at all.
///
/// A closed vocabulary, and not a product list: the platform is a path
/// segment in an immutable coordinate, and an operator-supplied segment is an
/// operator-supplied path. These are the two
/// [`crate::deploy::bootstrap::REMOTE_INSTALL_SCRIPT`] maps the remote kernel
/// and architecture onto, so a host cannot be described by a word the
/// installer does not know. Which of them a given product actually publishes
/// for is per product, and declared.
pub const PLATFORMS: &[&str] = &["darwin-arm64", "linux-amd64"];

/// launchd, the unit system on the macOS hosts.
pub const UNIT_LAUNCHD: &str = "launchd";
/// `systemd --user`, the unit system on the Linux hosts.
pub const UNIT_SYSTEMD: &str = "systemd";

/// The placeholder a unit label may carry for the host's registry name, so a
/// per-host label is declared once instead of once per host.
pub const TARGET_PLACEHOLDER: &str = "{target}";
