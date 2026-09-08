//! Declared web products and how one declaration is parsed.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use self::kind::parse_web_api_kind;
use self::placement::parse_web_api_placement;
use self::unit::parse_web_api_unit;
use crate::config::canonical_machine_name;
use serde_json::Value;

mod accessors;
mod kind;
mod placement;
mod unit;

/// One declared web product: the release it runs, where it runs, the identity
/// it runs as, and the hostname it answers on.
///
/// The declaration holds no secret value. `secrets` and `database` name a
/// Skarbiec item and one of its fields; the value travels only through
/// `stado service secret-sync`, which reads it over the host channel and puts
/// it in the unit's env file without it ever reaching a command line.
///
/// A product that declares `redirect_to` is a hostname and nothing else: the
/// edge answers it with a redirect, and there is no unit, no release and no
/// host. Five of the fleet's Vercel projects are exactly that — one rewrite
/// each to `https://wisent-app.com` — and expressing them as a product with a
/// port and a consumer would mean declaring a unit nobody runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebApiProduct {
    host: String,
    port: u16,
    hostname: String,
    consumer: String,
    readyz: String,
    edge: String,
    env: BTreeMap<String, String>,
    secrets: BTreeMap<String, String>,
    database: Option<WebApiDatabase>,
    redirect_to: Option<String>,
    /// A registry service this hostname is published in front of, instead of
    /// a unit this product owns.
    ///
    /// `brama.wisent.com` is that: Brama already runs as a managed service on
    /// the mini, it is not a Node web product, and nothing about it should be
    /// built or installed by `stado web`. What was missing was a public
    /// hostname with a certificate, which is the one thing the edge does.
    upstream_service: Option<String>,
    /// A path prefix this product is mounted at, under a hostname another
    /// declaration owns.
    ///
    /// `brama.wisent.com/docs` is that: Brama's 79 documentation pages are
    /// versioned with Brama and served by a unit of their own, while the
    /// hostname's catch-all belongs to Brama itself. A mount is an ordinary
    /// unit product in every other way — built, released and deployed like
    /// any other — and only its place in the edge's configuration differs.
    path_prefix: Option<String>,
}

/// The one database a web product reads, and how its credential reaches the
/// unit: the declared database name, the field of that database's Skarbiec
/// item, and the variable the field is delivered as.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebApiDatabase {
    name: String,
    field: String,
    variable: String,
}

pub(crate) fn parse_web_api_products(
    value: Option<&Value>,
) -> Result<BTreeMap<String, WebApiProduct>, Vec<String>> {
    let Some(Value::Object(entries)) = value else {
        return Err(vec![
            "web_api.products must be a non-empty object mapping product names to declarations"
                .to_string(),
        ]);
    };
    if entries.is_empty() {
        return Err(vec!["web_api.products must not be empty".to_string()]);
    }
    let mut problems = Vec::new();
    let mut products = BTreeMap::new();
    let mut hostnames = BTreeMap::new();
    let mut mounts = BTreeMap::new();
    for (name, raw) in entries {
        let start = problems.len();
        if !canonical_machine_name(name) {
            problems.push(format!("web_api.products key {name:?} is not canonical"));
        }
        let Some(entry) = raw.as_object() else {
            problems.push(format!(
                "web_api.products.{name} must be an object with host, port, hostname and consumer"
            ));
            continue;
        };
        let (redirect_to, upstream_service) = parse_web_api_kind(name, entry, &mut problems);
        let (host, port, hostname, path_prefix) = parse_web_api_placement(
            name,
            entry,
            redirect_to.as_deref(),
            upstream_service.as_deref(),
            &mut problems,
        );
        if !hostname.is_empty() {
            match &path_prefix {
                // The owner: one per hostname, holding the record, the
                // certificate and the catch-all.
                None => {
                    if let Some(owner) = hostnames.insert(hostname.clone(), name.clone()) {
                        problems.push(format!(
                            "web_api.products.{name}.hostname {hostname:?} is already declared by {owner:?}"
                        ));
                    }
                }
                // A mount: it shares the hostname, so the only thing it must
                // not share is its prefix. Two mounts at one prefix would
                // render two `handle_path` blocks for one path and the first
                // would silently win.
                Some(prefix) => {
                    if let Some(owner) =
                        mounts.insert((hostname.clone(), prefix.clone()), name.clone())
                    {
                        problems.push(format!(
                            "web_api.products.{name} mounts {prefix:?} on {hostname:?}, which {owner:?} already mounts"
                        ));
                    }
                }
            }
        }
        let unit = parse_web_api_unit(
            name,
            entry,
            redirect_to.as_deref(),
            upstream_service.as_deref(),
            &mut problems,
        );
        if problems.len() == start {
            products.insert(
                name.clone(),
                WebApiProduct {
                    host,
                    port,
                    hostname,
                    consumer: unit.consumer,
                    readyz: unit.readyz,
                    edge: unit.edge,
                    env: unit.env,
                    secrets: unit.secrets,
                    database: unit.database,
                    redirect_to,
                    upstream_service,
                    path_prefix,
                },
            );
        }
    }
    // A mount is rendered inside the site block of the declaration that owns
    // its hostname, so a mount with no owner is a block with nowhere to go:
    // the edge would order no certificate for that name and the path would
    // answer from nothing. Checked after the loop because the owner may be
    // declared after the mount in the document.
    for ((hostname, prefix), name) in &mounts {
        if !hostnames.contains_key(hostname) {
            problems.push(format!(
                "web_api.products.{name} mounts {prefix:?} on {hostname:?}, which no declaration owns: one product must declare that hostname without a path_prefix, and it is the one that holds the record and the certificate"
            ));
        }
    }
    if problems.is_empty() {
        Ok(products)
    } else {
        Err(problems)
    }
}

static WEB_API_PRODUCTS: LazyLock<Result<BTreeMap<String, WebApiProduct>, Vec<String>>> =
    LazyLock::new(|| {
        match std::env::var("WC_WEB_API_PRODUCTS")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(encoded) => serde_json::from_str::<Value>(&encoded)
                .map_err(|error| vec![format!("WC_WEB_API_PRODUCTS must be valid JSON: {error}")])
                .and_then(|parsed| parse_web_api_products(Some(&parsed))),
            None => parse_web_api_products(crate::config_file::get("web_api.products").as_ref()),
        }
    });

pub fn web_api_products() -> Result<&'static BTreeMap<String, WebApiProduct>, &'static [String]> {
    match &*WEB_API_PRODUCTS {
        Ok(products) => Ok(products),
        Err(problems) => Err(problems.as_slice()),
    }
}
