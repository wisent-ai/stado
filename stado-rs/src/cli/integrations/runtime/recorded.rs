//! The `stado serve` line of this machine's resident unit, rendered from two
//! sources: the product catalog, which owns the command line every host's
//! unit starts with, and this machine's record, which names the roles this
//! host runs beyond it (a resolver, a worker for its target, release and
//! product-sync loops). Rendering from the catalog alone would drop every
//! such role; rendering from the record alone would keep an option a later
//! release retired, and the unit would restart on it without end.

use clap::Args;

use super::ServeArgs;

/// One option of a recorded line: its long name and every word it spans.
struct Recorded {
    name: String,
    words: Vec<String>,
}

/// The long name `word` sets when it is an option, `--name` or `--name=value`.
fn long_name(word: &str) -> Option<&str> {
    let named = word.strip_prefix("--")?;
    match named.split_once('=') {
        Some((name, _value)) => Some(name),
        None => Some(named),
    }
}

/// The options of `line`, each with the value word that follows it when this
/// build's `serve` says the option takes one and it was not attached with
/// `=`. Words that are no option (the `serve` verb) are not options.
fn options(line: &[String], serve: &clap::Command) -> Vec<Recorded> {
    let mut found = Vec::new();
    let mut words = line.iter().peekable();
    while let Some(word) = words.next() {
        let Some(name) = long_name(word) else {
            continue;
        };
        let mut spanned = vec![word.clone()];
        let takes_value = serve
            .get_arguments()
            .find(|argument| argument.get_long() == Some(name))
            .is_some_and(|argument| argument.get_action().takes_values());
        if takes_value && !word.contains('=') {
            if let Some(value) = words.next_if(|next| long_name(next).is_none()) {
                spanned.push(value.clone());
            }
        }
        found.push(Recorded {
            name: name.to_string(),
            words: spanned,
        });
    }
    found
}

/// The resident unit's `serve` line for `product`: the catalog's `catalog`
/// line, then every option of the machine's `recorded` line the catalog line
/// does not set. An option this build's `serve` no longer has is left out
/// and named on stderr, so the unit starts and the operator reads which
/// recorded role this release retired. A product other than the host process
/// keeps the catalog's line as it is: only `serve`'s own parser can tell its
/// options.
pub(crate) fn serve_line(
    product: &str,
    catalog: Vec<String>,
    recorded: &[String],
) -> Result<Vec<String>, String> {
    if crate::deploy::service_catalog::host_process()?.name != product {
        return Ok(catalog);
    }
    let serve = ServeArgs::augment_args(clap::Command::new("serve"));
    let set: Vec<String> = options(&catalog, &serve)
        .into_iter()
        .map(|option| option.name)
        .collect();
    let mut line = catalog;
    for option in options(recorded, &serve) {
        if set.contains(&option.name) {
            continue;
        }
        let known = serve
            .get_arguments()
            .any(|argument| argument.get_long() == Some(option.name.as_str()));
        if known {
            line.extend(option.words);
        } else {
            eprintln!(
                "{product}: this machine's record runs `{}`, which is not an option of this \
                 build's serve; the unit is rendered without it",
                option.words.join(" ")
            );
        }
    }
    Ok(line)
}
