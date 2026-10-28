//! One onboarding screen as the CLI prints it.

use serde_json::Value;

/// Print `screen` as the `position`-th of `total`.
pub(super) fn render(screen: &Value, position: usize, total: usize) {
    let presentation = &screen["presentation"];
    // A screen whose definition gives no title or body says which: a blank
    // line read as a screen meant to be blank.
    let declared = |field: &str| match presentation[field].as_str() {
        Some(text) => text.to_string(),
        None => format!("(the onboarding definition gives this screen no {field})"),
    };
    println!(
        "{}/{}  {}\n       {}",
        position,
        total,
        declared("title"),
        declared("body")
    );
    if let Some(command) = presentation.get("command").and_then(Value::as_str) {
        println!("       $ {command}");
    }
    if let Some(result) = presentation.get("result").and_then(Value::as_str) {
        println!("       Result: {result}");
    }
    println!();
}
