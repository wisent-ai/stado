//! What kind of web product the checkout is, decided by its `start` script
//! and by nothing else.

use serde_json::{Map, Value};

use crate::cli::web::builds::contract::package::script;

/// What kind of web product the checkout is, decided by one thing.
///
/// **The rule: a product that declares a `start` script is a server, and a
/// product that does not is a static site.** Nothing else is consulted — not
/// the presence of `next.config.*`, not a dependency on `next`, not which
/// directories exist. `start` is what the launcher runs, so the question
/// "is there a server to start" and the question "what does this product
/// declare" have to be the same question. A product with a `build` and no
/// `start` is a site whose build writes files; a product with neither is a
/// site whose files are committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::cli::web::builds) enum Kind {
    /// Next.js or another Node server: `npm run build`, then `npm run start`.
    Server,
    /// A directory of files, served by the static server this build stages.
    Static,
}

impl Kind {
    pub(in crate::cli::web::builds) fn of(manifest: Option<&Map<String, Value>>) -> Self {
        match manifest {
            Some(manifest) if script(manifest, "start").is_some() => Self::Server,
            _ => Self::Static,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_script_is_what_makes_a_product_a_server() {
        // The whole rule, in one place. `byk-landing` has no package.json at
        // all; `tama-landing` has a build and no start; `preferences-landing`
        // has both. Nothing else — not next.config, not a dependency on next
        // — is allowed to answer this question, because `start` is the only
        // thing the launcher can run.
        assert_eq!(Kind::of(None), Kind::Static);
        let building = serde_json::json!({ "scripts": { "build": "node scripts/build.mjs" } });
        assert_eq!(Kind::of(building.as_object()), Kind::Static);
        let served =
            serde_json::json!({ "scripts": { "build": "next build", "start": "next start" } });
        assert_eq!(Kind::of(served.as_object()), Kind::Server);
        // A whitespace `start` runs nothing, so it does not make a server.
        let empty = serde_json::json!({ "scripts": { "start": "  " } });
        assert_eq!(Kind::of(empty.as_object()), Kind::Static);
    }
}
