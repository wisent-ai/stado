//! macOS: `pmset -g log` for sleep/wake, `log show` for interface changes.
//!
//! Which of configd's unified-log messages record an interface change is
//! read by a model through Oko's one-shot call (`link-prompt.md` beside this
//! file): configd states link, carrier and Wi-Fi association changes in
//! sentences, not in a field, and a predicate on the words `link` and `SSID`
//! dropped every change worded otherwise and reported every message that
//! happened to contain the word. The messages are deduplicated with their
//! digits masked before they are asked about, so one judgement covers every
//! repetition of a sentence; the entries kept are the ones whose masked
//! sentence the model named.

use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::Value;

use crate::deploy::host_link::probes::{probe, resolve_program, window_minutes};
use crate::deploy::host_link::timestamps::{detail_of, iso, newest_changes, parse_stamp};
use crate::deploy::host_link::InterfaceChange;
use crate::deploy::Runner;
use crate::primitives::model_judgement;

/// The `pmset -g log` transition kind that begins a sleep, and the word
/// every kind that ends one ends with (`Wake`, and `DarkWake`, a maintenance
/// wake that is still the moment the network came back).
const SLEEP_KIND: &str = "Sleep";
const WAKE_SUFFIX: &str = "Wake";

/// The unified-log predicate: configd's own entries. The process anchor is
/// what lets the log store skip; an unanchored predicate over hours of log
/// is minutes of work.
const CONFIGD_PREDICATE: &str = "process == \"configd\"";

const PROMPT: &str = include_str!("link-prompt.md");
const MESSAGES_SLOT: &str = "{messages}";

/// `(last_sleep_at, last_wake_at)` from `pmset -g log`, or `None` when pmset
/// is absent or failed.
///
/// The read is bounded from the end: the log holds tens of thousands of lines
/// and only the newest transition of each kind is wanted, so the scan walks
/// backwards and stops as soon as it has both.
pub(in crate::deploy::host_link) async fn macos_sleep_wake(
    runner: &Runner,
) -> Option<(Option<String>, Option<String>)> {
    let program = resolve_program("pmset")?;
    let output = probe(runner, vec![program, "-g".to_string(), "log".to_string()]).await?;

    let mut sleep = None;
    let mut wake = None;
    for line in output.stdout.lines().rev() {
        let mut fields = line.split_whitespace();
        let (Some(date), Some(time), Some(offset), Some(kind)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let is_sleep = kind == SLEEP_KIND;
        let is_wake = kind.ends_with(WAKE_SUFFIX);
        if (is_sleep && sleep.is_some()) || (is_wake && wake.is_some()) || (!is_sleep && !is_wake) {
            continue;
        }
        let Some(stamp) = parse_stamp(&format!("{date} {time} {offset}")) else {
            continue;
        };
        if is_sleep {
            sleep = Some(iso(stamp));
        } else {
            wake = Some(iso(stamp));
        }
        if sleep.is_some() && wake.is_some() {
            break;
        }
    }
    Some((sleep, wake))
}

/// A message with every digit masked, so repetitions share one sentence.
fn masked(message: &str) -> String {
    message
        .chars()
        .map(|character| {
            if character.is_ascii_digit() {
                '#'
            } else {
                character
            }
        })
        .collect()
}

/// The model's answer: the sentences that record an interface change.
#[derive(Deserialize)]
struct Judged {
    changes: Vec<String>,
}

/// Which of `sentences` record an interface change, as the model reads them.
fn judged_changes(sentences: &BTreeSet<String>) -> Result<BTreeSet<String>, String> {
    let listed = serde_json::to_string(sentences)
        .map_err(|error| format!("the configd sentences could not be listed: {error}"))?;
    let answer = model_judgement::ask(&PROMPT.replace(MESSAGES_SLOT, &listed))?;
    let judged: Judged = serde_json::from_str(answer.trim())
        .map_err(|error| format!("the model's answer is not the asked JSON ({error}): {answer}"))?;
    Ok(judged.changes.into_iter().collect())
}

/// Interface changes inside the window from the unified log, or `None` when
/// `log` is absent or failed or its messages could not be judged, the cause
/// said. `Some(vec![])` means the window was quiet.
pub(in crate::deploy::host_link) async fn macos_interface_changes(
    runner: &Runner,
    window: i64,
) -> Option<Vec<InterfaceChange>> {
    let program = resolve_program("log")?;
    let output = probe(
        runner,
        vec![
            program,
            "show".to_string(),
            "--last".to_string(),
            format!("{}m", window_minutes(window)),
            "--style".to_string(),
            "ndjson".to_string(),
            "--predicate".to_string(),
            CONFIGD_PREDICATE.to_string(),
        ],
    )
    .await?;

    let entries: Vec<_> = output
        .stdout
        .lines()
        .filter_map(|line| {
            let entry: Value = serde_json::from_str(line.trim()).ok()?;
            let stamp = parse_stamp(entry.get("timestamp")?.as_str()?)?;
            let message = detail_of(entry.get("eventMessage")?.as_str()?);
            (!message.is_empty()).then_some((stamp, message))
        })
        .collect();
    let sentences: BTreeSet<String> = entries.iter().map(|(_, message)| masked(message)).collect();
    if sentences.is_empty() {
        return Some(Vec::new());
    }
    let judged = match tokio::task::spawn_blocking(move || judged_changes(&sentences)).await {
        Ok(Ok(judged)) => judged,
        Ok(Err(cause)) => {
            eprintln!("host link: configd's messages were not judged: {cause}");
            return None;
        }
        Err(error) => {
            eprintln!("host link: judging configd's messages stopped: {error}");
            return None;
        }
    };
    let changes = entries
        .into_iter()
        .filter(|(_, message)| judged.contains(&masked(message)))
        .collect();
    Some(newest_changes(changes))
}
