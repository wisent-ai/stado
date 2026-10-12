//! One model judgement, asked through Oko's one-shot model call.
//!
//! Where Stado has to decide what a piece of prose means — what a provider's
//! mail asks of the operator, what a vendor's reply says about a quota — a
//! list of phrases decides by wording, in one language, and misses every
//! sentence it did not foresee. The honest decider is a model, and every
//! model call on the fleet goes through Brama. Oko owns the bearer and the
//! route for a one-shot call (`oko gateway send --prompt`, printing the
//! service's answer as JSON with the OpenAI-shaped reply under `body`), so
//! Stado asks through it rather than holding a model credential of its own.
//!
//! A judgement nobody could obtain is an error naming why: the caller says
//! what it could not read, and never treats the silence as an answer.

use std::process::Command;

use serde_json::Value;

/// The Oko program and the command that sends one prompt.
const OKO_CLI: &str = "oko";
const SEND_ARGUMENTS: &[&str] = &["gateway", "send", "--json", "--prompt"];
/// Where the service's reply sits in Oko's answer, and the answer's text in
/// the reply.
const BODY_KEY: &str = "body";
const CHOICES_KEY: &str = "choices";
const MESSAGE_KEY: &str = "message";
const CONTENT_KEY: &str = "content";

/// The text of the model's answer inside one `oko gateway send` output.
fn answer_text(stdout: &str) -> Result<String, String> {
    let answer: Value = serde_json::from_str(stdout.trim())
        .map_err(|error| format!("{OKO_CLI} gateway send answered no JSON ({error}): {stdout}"))?;
    let body = answer
        .get(BODY_KEY)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{OKO_CLI} gateway send answered without a body: {stdout}"))?;
    let reply: Value = serde_json::from_str(body)
        .map_err(|error| format!("the model service answered no JSON ({error}): {body}"))?;
    let text = reply
        .get(CHOICES_KEY)
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get(MESSAGE_KEY))
        .and_then(|message| message.get(CONTENT_KEY))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| format!("the model service answered with no message: {body}"))?;
    Ok(text.to_string())
}

/// Ask Brama one question through Oko and return what the model said.
///
/// `prompt` is the whole prompt, including the answer format the caller
/// requires; the caller keeps its own contract with the model.
pub fn ask(prompt: &str) -> Result<String, String> {
    let output = crate::wait::output(Command::new(OKO_CLI).args(SEND_ARGUMENTS).arg(prompt))
        .map_err(|error| format!("{OKO_CLI} gateway send did not run: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{OKO_CLI} gateway send failed ({}): {}{}",
            output.status,
            stderr.trim(),
            stdout.trim()
        ));
    }
    answer_text(&stdout)
}
