//! The rendered file: what one product, several, a redirect and an empty edge
//! each produce, and what reads back out of them.

use super::fixtures::{blocks, edge};
use super::*;
use crate::cli::web::edge::serving::directives::{redirect, route};

#[test]
fn one_product_becomes_one_site_block_over_the_tailnet() {
    let routes = vec![route("preferences.wisent.com", "charless-mac-mini", 3210).unwrap()];
    let rendered = caddyfile(&edge(), &blocks(routes));
    assert!(
        rendered.contains("\temail operator@wisent.com\n"),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "\npreferences.wisent.com {\n\treverse_proxy http://charless-mac-mini:\
             3210\n}\n"
        ),
        "{rendered}"
    );
    assert_eq!(
        terminated_hostnames(&rendered),
        vec!["preferences.wisent.com".to_string()]
    );
}

#[test]
fn a_redirect_product_becomes_a_redir_block_and_no_proxy() {
    let routes = vec![redirect("aiwisent.com", "https://wisent-app.com").unwrap()];
    let rendered = caddyfile(&edge(), &blocks(routes));
    // `{uri}` carries the path and the query across, which is what the
    // Vercel rewrite these replace did with `/:path*`. 308 keeps the
    // method, so a POST does not silently become a GET.
    assert!(
        rendered.contains("\naiwisent.com {\n\tredir https://wisent-app.com{uri} 308\n}\n"),
        "{rendered}"
    );
    // No unit is involved, so nothing is proxied anywhere.
    assert!(!rendered.contains("reverse_proxy"), "{rendered}");
    // The edge still terminates the hostname, so it still orders the
    // certificate for it.
    assert_eq!(
        terminated_hostnames(&rendered),
        vec!["aiwisent.com".to_string()]
    );
}

#[test]
fn redirects_and_proxies_share_one_configuration() {
    let mut routes = vec![
        redirect("wisentai.com", "https://wisent-app.com").unwrap(),
        route("preferences.wisent.com", "charless-mac-mini", 3210).unwrap(),
    ];
    routes.sort();
    let rendered = caddyfile(&edge(), &blocks(routes));
    assert_eq!(
        rendered.matches("\treverse_proxy ").count(),
        1,
        "{rendered}"
    );
    assert_eq!(rendered.matches("\tredir ").count(), 1, "{rendered}");
    assert_eq!(terminated_hostnames(&rendered).len(), 2, "{rendered}");
}

#[test]
fn a_redirect_target_that_could_break_the_generated_file_is_refused() {
    // http would send a browser from a hostname this edge holds a
    // certificate for to one it does not. A query or a fragment would
    // collide with the appended `{uri}`. A brace is the one placeholder
    // syntax in the generated file and it belongs to Stado. A trailing
    // slash would make every redirected path a double slash.
    for target in [
        "http://wisent-app.com",
        "https://wisent-app.com?a=1",
        "https://wisent-app.com#top",
        "https://wisent-app.com/",
        "https://wisent-app.com{uri}",
        "https://wisent app.com",
        "wisent-app.com",
        "",
    ] {
        let refused = redirect("aiwisent.com", target)
            .expect_err(&format!("{target:?} must not reach the Caddyfile"));
        assert!(refused.message.is_some_and(|message| !message.is_empty()));
    }
    // A path prefix is a real thing to want and is allowed.
    redirect("aiwisent.com", "https://wisent-app.com/pricing").unwrap();
}

#[test]
fn several_products_each_get_their_own_site_block() {
    let routes = vec![
        route("app.preferences.wisent.com", "charless-mac-mini", 3211).unwrap(),
        route("preferences.wisent.com", "charless-mac-mini", 3210).unwrap(),
        route("needher.needher.ai", "ubuntu-server-rtx-pro-6000", 3400).unwrap(),
    ];
    let rendered = caddyfile(&edge(), &blocks(routes));
    // One global block, and one site block per product.
    assert_eq!(
        rendered.matches("\treverse_proxy ").count(),
        3,
        "{rendered}"
    );
    assert_eq!(rendered.matches("\temail ").count(), 1, "{rendered}");
    assert!(
        rendered.contains(
            "\nneedher.needher.ai {\n\treverse_proxy http://ubuntu-server-rtx-pro-6000:3400\n}\n"
        ),
        "{rendered}"
    );
    // The rendered file reads back as exactly the set it was built from,
    // which is what makes the reconcile a comparison and not a guess.
    assert_eq!(
        terminated_hostnames(&rendered),
        vec![
            "app.preferences.wisent.com".to_string(),
            "needher.needher.ai".to_string(),
            "preferences.wisent.com".to_string(),
        ]
    );
}

#[test]
fn an_empty_edge_still_renders_a_valid_file_and_terminates_nothing() {
    let rendered = caddyfile(&edge(), &[]);
    assert!(
        rendered.contains("\temail operator@wisent.com\n"),
        "{rendered}"
    );
    assert!(terminated_hostnames(&rendered).is_empty(), "{rendered}");
}

#[test]
fn a_hostname_that_is_not_a_public_host_name_is_refused() {
    for candidate in [
        "localhost",
        "Preferences.Wisent.com",
        "preferences.wisent.com.",
        "preferences wisent.com {",
        "",
    ] {
        let refused = route(candidate, "charless-mac-mini", 3210)
            .expect_err("a name no certificate can be ordered for must be refused");
        let message = refused.message.unwrap_or_default();
        assert!(
            message.contains("is not a public host name"),
            "{candidate:?}: {message}"
        );
    }
}

#[test]
fn an_upstream_that_is_not_a_tailnet_name_or_port_is_refused() {
    let refused = route("preferences.wisent.com", "charless mac mini", 3210)
        .expect_err("a host name that would break the generated file must be refused");
    assert!(refused
        .message
        .unwrap_or_default()
        .contains("is not a tailnet host name"),);
    let refused = route("preferences.wisent.com", "charless-mac-mini", 0)
        .expect_err("a port nothing listens on must be refused");
    assert!(refused.message.unwrap_or_default().contains("port 0"));
}

#[test]
fn the_global_block_and_indented_directives_are_not_site_addresses() {
    let rendered = caddyfile(
        &edge(),
        &blocks(vec![route("a.wisent.com", "host", 80).unwrap()]),
    );
    assert_eq!(
        terminated_hostnames(&rendered),
        vec!["a.wisent.com".to_string()]
    );
    // A comment naming a hostname is a comment, not a site block.
    let commented = "# preferences.wisent.com {\n{\n\temail a@b.com\n}\n";
    assert!(terminated_hostnames(commented).is_empty(), "{commented}");
}
