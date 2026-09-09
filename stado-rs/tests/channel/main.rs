//! The public release-channel boundary, driven against the machine running
//! these tests.
//!
//! `/docs/channels` keeps the last of its five boundaries separate on purpose:
//! "public object and release HTTP". A host may answer while its public name
//! does not exist, and a node may publish a perfect handler table for a name
//! no resolver can find. This area is that boundary's declaration and its
//! reality check.
//!
//! Every case here names one host: this one. The registry target declares the
//! kernel's own answer to `/bin/hostname`, so the product takes its
//! current-host path; the origin under judgement is this machine's own name,
//! which is the honest hard case, because a workstation answers to that name
//! on its own network and no public resolver has ever heard of it. Upstreams
//! are loopback sockets the cases really bind, the publication reading comes
//! out of this node's own tailscale handler table, and the edge reading is
//! taken from the product's own HTTP service running on this machine's
//! loopback or from a port nothing is listening on. Every refusal sentence
//! asserted here was copied from a live run of the built binary.
//!
//! The files are split so each stays inside the three hundred line limit this
//! repository enforces on itself:
//!
//! - [`fixture`]: the isolated canonical registry, the product invocation and
//!   the readers of the persisted document.
//! - [`listeners`]: the loopback upstream, the product's own service, and the
//!   probe for this machine's tailscale CLI.
//! - [`declaration`]: what the registry accepts and refuses, and what it
//!   leaves on disk.
//! - [`edge`]: the resolution, publication and edge readings a verdict is
//!   composed from.
//! - [`release`]: the public ingress journey, run by the repository's test
//!   runner against a real release coordinate.

mod declaration;
mod edge;
mod fixture;
mod listeners;
mod release;
