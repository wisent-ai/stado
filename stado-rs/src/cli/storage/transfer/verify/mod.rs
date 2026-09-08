//! `stado storage verify`: how two stores compare, object for object.

use crate::cli::storage::*;

pub(in crate::cli::storage) mod diff;
pub(in crate::cli::storage) mod report;

// ---- verify ----

#[derive(Args, Debug)]
pub struct StorageVerifyArgs {
    #[command(flatten)]
    ends: EndpointArgs,

    /// Restrict the comparison to this prefix. Repeatable. Omit to compare
    /// the whole canonical prefix set.
    #[arg(long = "prefix")]
    prefix: Vec<String>,
    #[arg(long)]
    json: bool,
}

/// How one prefix compares across the two stores.
#[derive(Default)]
pub(in crate::cli::storage) struct PrefixDiff {
    prefix: String,
    /// `None` when that side could not be listed: unknown, not empty.
    source_objects: Option<usize>,
    destination_objects: Option<usize>,
    missing: Vec<String>,
    extra: Vec<String>,
    metadata_gaps: Vec<(String, Vec<String>)>,
    body_mismatches: Vec<String>,
    body_errors: Vec<(String, String)>,
    source_error: Option<String>,
    destination_error: Option<String>,
}

impl PrefixDiff {
    fn diverged(&self) -> bool {
        self.source_error.is_some()
            || self.destination_error.is_some()
            || !self.missing.is_empty()
            || !self.extra.is_empty()
            || !self.metadata_gaps.is_empty()
            || !self.body_mismatches.is_empty()
            || !self.body_errors.is_empty()
    }

    fn status(&self) -> String {
        if let Some(error) = &self.source_error {
            return format!("SOURCE UNREADABLE: {error}");
        }
        if let Some(error) = &self.destination_error {
            return format!("DESTINATION UNREADABLE: {error}");
        }
        if self.diverged() {
            return "DIVERGED".to_string();
        }
        "match".to_string()
    }
}

/// The prefixes a comparison walks: the explicit selection, or the
/// canonical set when none was given. Mirrors
/// `queue/copy.rs::selected_prefixes` so `storage verify` covers exactly
/// what `storage copy` moves.
fn selected_prefixes(requested: &[String]) -> Vec<String> {
    if requested.is_empty() {
        return CANONICAL_PREFIXES
            .iter()
            .map(|prefix| (*prefix).to_string())
            .collect();
    }
    requested.to_vec()
}

/// Full post-copy verification. Reads names, metadata, and body bytes from
/// both stores and writes to neither; exits non-zero on any divergence.
pub(in crate::cli::storage) async fn verify(args: &StorageVerifyArgs) -> Result<(), CmdError> {
    verify_between(
        args.ends.source(),
        args.ends.destination(),
        &args.prefix,
        args.json,
    )
    .await
}

pub(crate) async fn verify_between(
    from: Endpoint,
    to: Endpoint,
    requested_prefixes: &[String],
    as_json: bool,
) -> Result<(), CmdError> {
    if from.describe() == to.describe() {
        return Err(CmdError::click(format!(
            "source and destination are the same store ({}); there is nothing to compare",
            from.describe()
        )));
    }
    let source = from.build().await?;
    let destination = to.build().await?;
    let prefixes = selected_prefixes(requested_prefixes);

    let diffs: Vec<PrefixDiff> = futures::stream::iter(prefixes.iter())
        .map(|prefix| diff_prefix(&source, &destination, prefix))
        .buffered(copy::DEFAULT_CONCURRENCY)
        .collect()
        .await;

    let missing: usize = diffs.iter().map(|diff| diff.missing.len()).sum();
    let extra: usize = diffs.iter().map(|diff| diff.extra.len()).sum();
    let gaps: usize = diffs.iter().map(|diff| diff.metadata_gaps.len()).sum();
    let body_mismatches: usize = diffs.iter().map(|diff| diff.body_mismatches.len()).sum();
    let body_errors: usize = diffs.iter().map(|diff| diff.body_errors.len()).sum();
    let diverging = diffs.iter().filter(|diff| diff.diverged()).count();
    let divergent = diffs.iter().any(PrefixDiff::diverged);

    if as_json {
        echo_json(&json!({
            "from": from.describe(),
            "to": to.describe(),
            "prefixes": diffs.iter().map(diff_json).collect::<Vec<Value>>(),
            "missing_at_destination": missing,
            "only_at_destination": extra,
            "metadata_mismatches": gaps,
            "body_mismatches": body_mismatches,
            "body_read_errors": body_errors,
            "diverging_prefixes": diverging,
            "divergent": divergent,
        }))?;
    } else {
        println!(
            "{} -> {} (read-only; nothing is written)",
            from.describe(),
            to.describe()
        );
        print_diff_table(&diffs);
        print_diff_detail(&diffs);
    }

    if !divergent {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{diverging} of {} prefix(es) diverge: {missing} object(s) missing at the \
         destination, {extra} only at the destination, {gaps} whose metadata did not \
         land, {body_mismatches} with different content, {body_errors} with unreadable \
         content. Nothing was copied — re-run `stado storage copy` with the same locators, \
         then verify again.",
        diffs.len()
    )))
}
