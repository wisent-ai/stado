//! The spellings a private GitHub dependency is reached by, built from the
//! one declaration of GitHub's Git origin in `github.json` beside this file:
//! the host, the user its SSH transport signs in as, and the user an
//! installation token authenticates as over HTTPS. Every rewrite and every
//! check of a lockfile reads them here, so no file spells an origin of its own.

use std::sync::LazyLock;

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Declared {
    host: String,
    ssh_user: String,
    token_user: String,
}

static DECLARED: LazyLock<Declared> = LazyLock::new(|| {
    serde_json::from_str(include_str!("github.json"))
        .expect("github.json beside this file declares GitHub's Git origin")
});

/// The SSH URL prefix npm hands to Git for a `github:` dependency.
pub(in crate::cli::web::builds) fn ssh() -> String {
    format!("ssh://{}@{}/", DECLARED.ssh_user, DECLARED.host)
}

/// The scp-like spelling Git also reaches GitHub by.
pub(in crate::cli::web::builds) fn scp() -> String {
    format!("{}@{}:", DECLARED.ssh_user, DECLARED.host)
}

/// The HTTPS prefix that fetches with `token`.
pub(in crate::cli::web::builds) fn https_with(token: &str) -> String {
    format!("https://{}:{token}@{}/", DECLARED.token_user, DECLARED.host)
}
