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
//!
//! A decision between named options is asked with [`choose`]: the model
//! answers one object bound to a JSON Schema whose `choice` is one of the
//! options (`oko gateway object`), so no caller reads the first word of a
//! line of prose, where a model that framed its answer otherwise was read
//! as the other option.

use std::process::Command;

use serde::Deserialize;
use serde_json::{json, Value};

/// The Oko program and the commands that send one prompt, and one prompt
/// whose answer is bound to a schema.
const OKO_CLI: &str = "oko";
const SEND_ARGUMENTS: &[&str] = &["gateway", "send", "--json", "--prompt"];
const OBJECT_ARGUMENTS: &[&str] = &["gateway", "object", "--prompt"];
const NAME_FLAG: &str = "--name";
const SCHEMA_FLAG: &str = "--schema";
/// Where the service's reply sits in Oko's answer, and the answer's text in
/// the reply.
const BODY_KEY: &str = "body";
const CHOICES_KEY: &str = "choices";
const MESSAGE_KEY: &str = "message";
const CONTENT_KEY: &str = "content";
/// The two fields of a choice: the option chosen and the model's reason.
const CHOICE_KEY: &str = "choice";
const REASON_KEY: &str = "reason";

/// The option a model chose among those it was offered, and why.
#[derive(Debug, Deserialize)]
pub struct Choice {
    pub choice: String,
    pub reason: String,
}

/// The output of one Oko call that ran, or why it did not run or failed.
fn oko(arguments: &[&str], what: &str) -> Result<String, String> {
    let output = crate::wait::output(Command::new(OKO_CLI).args(arguments))
        .map_err(|error| format!("{OKO_CLI} {what} did not run: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{OKO_CLI} {what} failed ({}): {}{}",
            output.status,
            stderr.trim(),
            stdout.trim()
        ));
    }
    Ok(stdout)
}

/// The JSON object a model answers to `prompt`, bound to `schema` named
/// `name` with strict set; an answer that is not one object is the error.
pub fn object(prompt: &str, name: &str, schema: &Value) -> Result<Value, String> {
    let schema = schema.to_string();
    let mut arguments = OBJECT_ARGUMENTS.to_vec();
    arguments.extend([prompt, NAME_FLAG, name, SCHEMA_FLAG, schema.as_str()]);
    let stdout = oko(&arguments, "gateway object")?;
    match serde_json::from_str(stdout.trim()) {
        Ok(answer @ Value::Object(_)) => Ok(answer),
        Ok(other) => Err(format!(
            "{OKO_CLI} gateway object answered no {name} object: {other}"
        )),
        Err(error) => Err(format!(
            "{OKO_CLI} gateway object answered no JSON ({error}): {stdout}"
        )),
    }
}

/// The one of `options` a model chooses for `prompt`, with its reason: the
/// prompt says what each option means, and the answer is bound to a schema
/// that admits only those options. A choice outside them is the error.
pub fn choose(prompt: &str, name: &str, options: &[&str]) -> Result<Choice, String> {
    let schema = json!({
        "type": "object",
        "properties": {
            CHOICE_KEY: { "type": "string", "enum": options },
            REASON_KEY: { "type": "string" },
        },
        "required": [CHOICE_KEY, REASON_KEY],
        "additionalProperties": false,
    });
    let answer = object(prompt, name, &schema)?;
    let chosen: Choice = serde_json::from_value(answer.clone())
        .map_err(|error| format!("the {name} answer is not a choice ({error}): {answer}"))?;
    if !options.contains(&chosen.choice.as_str()) {
        return Err(format!(
            "the {name} answer chose {:?}, none of {}: {answer}",
            chosen.choice,
            options.join(", ")
        ));
    }
    Ok(chosen)
}

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
    let mut arguments = SEND_ARGUMENTS.to_vec();
    arguments.push(prompt);
    answer_text(&oko(&arguments, "gateway send")?)
}
