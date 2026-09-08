//! What the artifact carries: the launcher the managed unit executes, the
//! static server a site is served by, every path that goes in, and the
//! reproducible tarball they are written into.

pub(in crate::cli::web::builds) mod archive;
mod launcher;
mod members;
mod static_server;

/// The file name of the static server this build stages beside the launcher.
const STATIC_SERVER: &str = "bin/serve-static.mjs";

/// The directory a static site is staged under inside the artifact.
///
/// A constant rather than the product's own `--root`: the launcher then names
/// one path on every host, and nothing about the builder's layout — whether
/// the site was `dist/`, `web/` or the repository root — survives into the
/// thing that runs.
const STATIC_SITE_DIR: &str = "site";
