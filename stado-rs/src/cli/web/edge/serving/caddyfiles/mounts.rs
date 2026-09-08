//! The mount: the order it is rendered in, the prefixes it refuses, and every
//! page that has to resolve behind it.

use super::corpus::{BRAMA_DOCS_FILES, BRAMA_DOCS_ROUTES};
use super::fixtures::edge;
use super::*;
use crate::cli::web::edge::serving::directives::{mount, route};

/// The static server's resolution rule, applied to the same paths it will
/// be applied to in production: the exact file, then `<path>.html`, then
/// `<path>/index.html`.
///
/// The rule itself lives in the server `stado web build` stages, in
/// JavaScript, because that is where it runs. Asserting that file's text
/// would prove nothing about whether these pages resolve, so the three
/// steps are applied here — and
/// `cli::web::builds::payload::static_server::tests::the_static_server_fetches_nothing_and_stays_inside_the_site`
/// pins the server's own order to the same three, so the two cannot drift
/// apart unnoticed.
fn resolve_static(path: &str, files: &[&str]) -> Option<String> {
    let relative = path.trim_start_matches('/');
    let candidates = [
        relative.to_string(),
        format!("{relative}.html"),
        if relative.is_empty() {
            "index.html".to_string()
        } else {
            format!("{relative}/index.html")
        },
    ];
    candidates
        .into_iter()
        .find(|candidate| files.contains(&candidate.as_str()))
}

#[test]
fn a_mount_is_handled_before_the_owners_catch_all() {
    // The owner forwards the whole hostname to Brama's own unit; the mount
    // answers /docs from the documentation unit. The order is the
    // semantics: Caddy takes the first matching route in a block, so a
    // catch-all rendered first would answer /docs itself and the mount
    // would never be reached.
    let owner = route("brama.wisent.com", "charless-mac-mini", 18081).unwrap();
    let mounted = mount("brama.wisent.com", "/docs", "charless-mac-mini", 3220).unwrap();
    let routes = vec![(
        "brama.wisent.com".to_string(),
        vec![mounted.1.clone(), owner.1.clone()],
    )];
    let rendered = caddyfile(&edge(), &routes);
    let block = rendered
        .split_once("brama.wisent.com {")
        .expect("the hostname has a site block")
        .1;
    let handle = block
        .find("handle_path /docs* {")
        .expect("the mount is rendered as handle_path");
    let catch_all = block
        .find(
            "reverse_proxy http://charless-mac-mini:\
               18081",
        )
        .expect("the owner's directive is rendered");
    assert!(handle < catch_all, "{rendered}");
    // handle_path, not handle: the prefix is stripped before proxying, so
    // /docs/core arrives at the unit as /core. `handle` would forward it
    // unchanged and every page would answer 404.
    assert!(!block.contains("handle /docs"), "{rendered}");
    assert!(
        block.contains(
            "handle_path /docs* {\n\t\treverse_proxy http://charless-mac-mini:\
             3220\n\t}"
        ),
        "{rendered}"
    );
    // One site block, so one certificate, for the hostname the owner
    // declared.
    assert_eq!(
        terminated_hostnames(&rendered),
        vec!["brama.wisent.com".to_string()]
    );
}

#[test]
fn every_docs_route_the_ingress_rewrote_resolves_behind_the_mount() {
    // What `handle_path /docs*` leaves of each request, and what the static
    // server then makes of it. If one of these did not resolve, moving
    // brama.wisent.com onto this edge would take that page offline.
    let files: Vec<&str> = BRAMA_DOCS_FILES.to_vec();
    let mut checked = 0_usize;
    for source in BRAMA_DOCS_ROUTES {
        if let Some(pattern) = source.strip_suffix("/:slug") {
            // One rule stands for every file in a directory, so each file
            // is checked and not the pattern.
            let directory = pattern.trim_start_matches("/docs").trim_start_matches('/');
            let members: Vec<&str> = files
                .iter()
                .copied()
                .filter(|name| {
                    name.starts_with(&format!("{directory}/")) && name.ends_with(".html")
                })
                .collect();
            assert!(
                !members.is_empty(),
                "{source} stands for no file under {directory}/"
            );
            for name in members {
                let slug = name
                    .trim_start_matches(&format!("{directory}/"))
                    .trim_end_matches(".html");
                // The prefix the mount strips, leaving the unit's own path.
                let stripped = format!("/{directory}/{slug}");
                assert_eq!(
                    resolve_static(&stripped, &files).as_deref(),
                    Some(name),
                    "{source} -> {stripped} must resolve to {name}"
                );
                checked += 1;
            }
            continue;
        }
        let stripped = source.trim_start_matches("/docs");
        let stripped = if stripped.is_empty() { "/" } else { stripped };
        assert!(
            resolve_static(stripped, &files).is_some(),
            "{source} -> {stripped} resolves to nothing, so that page would 404"
        );
        checked += 1;
    }
    // Every page, and not fewer: a rule that quietly stopped covering half
    // of them would still pass an "each one that ran resolved" assertion.
    assert_eq!(checked, 79, "every documentation page must be checked");
    // /docs itself is the directory index, which is what the ingress
    // rewrote to /docs/index.html.
    assert_eq!(resolve_static("/", &files).as_deref(), Some("index.html"));
}

#[test]
fn a_mount_prefix_that_could_change_the_matcher_is_refused() {
    // The rendered matcher is `<prefix>*`. A trailing slash would stop
    // /docs itself from matching; a wildcard or a brace of its own would
    // rewrite the matcher into something nobody declared.
    for prefix in [
        "docs", "/docs/", "/", "", "/docs*", "/docs{x}", "/do cs", "/../etc",
    ] {
        mount("brama.wisent.com", prefix, "charless-mac-mini", 3220)
            .expect_err("{prefix:?} must not reach the Caddyfile");
    }
    mount("brama.wisent.com", "/docs/api", "charless-mac-mini", 3220).unwrap();
}
