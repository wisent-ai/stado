//! Boat prompt submission, prompt-run status and steer payload readers.

use serde_json::{json, Map, Value};

use super::super::types::{jbool, jstr, BoxError, BoxPromptRun, PromptStatus};
use super::BoxClient;

/// The `promptRun` object Boat's answer carries, refused by name when it is
/// absent or not an object.
fn prompt_run(value: &Map<String, Value>, answer: &str) -> Result<Map<String, Value>, BoxError> {
    match value.get("promptRun") {
        Some(Value::Object(run)) => Ok(run.clone()),
        Some(other) => Err(BoxError::transport(format!(
            "Boat answered {answer} with a promptRun that is not an object: {other}"
        ))),
        None => Err(BoxError::transport(format!(
            "Boat answered {answer} without its promptRun"
        ))),
    }
}

impl BoxClient {
    /// POST /sandboxes/{id}/prompt. The response must carry a `promptRun`
    /// object whose id matches the top-level `promptId` (when present).
    pub async fn prompt(
        &self,
        box_id: &str,
        prompt: &str,
        provider: &str,
        model: &str,
        reasoning_effort: &str,
    ) -> Result<BoxPromptRun, BoxError> {
        let mut body = json!({"provider": provider, "prompt": prompt});
        if !model.is_empty() {
            body["model"] = Value::from(model);
        }
        if !reasoning_effort.is_empty() {
            body["reasoningEffort"] = Value::from(reasoning_effort);
        }
        let value = self
            .transport
            .request_json(
                "POST",
                &format!("{}/prompt", Self::box_path(box_id)?),
                Some(&body),
                &[],
                &["prompt.queued"],
            )
            .await?;
        let run = prompt_run(&value, "the prompt")?;
        let prompt_id = super::super::types::first_stated(
            &[value.get("promptId"), run.get("promptId")],
            "",
        );
        let run_id = jstr(run.get("promptId"), "promptId", "prompt run")?;
        if prompt_id.is_empty() || run_id != prompt_id {
            return Err(BoxError::transport(format!(
                "Boat answered the prompt with prompt id {:?} beside promptRun id {:?}",
                prompt_id, run_id
            )));
        }
        // A submission Boat answers without a status is queued: that is the
        // state the `prompt.queued` answer names.
        let status = jstr(run.get("status"), "status", "prompt run")?;
        let status = if status.is_empty() {
            PromptStatus::Queued
        } else {
            PromptStatus::read(&status)?
        };
        Ok(BoxPromptRun {
            prompt_id,
            status,
            done: jbool(run.get("done"), "done", "prompt run")?,
            raw: run,
        })
    }

    /// GET /sandboxes/{id}/prompts/{prompt_id}.
    pub async fn prompt_status(
        &self,
        box_id: &str,
        prompt_id: &str,
    ) -> Result<BoxPromptRun, BoxError> {
        let value = self
            .transport
            .request_json(
                "GET",
                &format!("{}/prompts/{prompt_id}", Self::box_path(box_id)?),
                None,
                &[],
                &["prompt.run"],
            )
            .await?;
        let run = prompt_run(&value, "the prompt status")?;
        let returned_id = jstr(run.get("promptId"), "promptId", "prompt run")?;
        if returned_id != prompt_id {
            return Err(BoxError::transport(format!(
                "Boat answered the status of prompt {prompt_id:?} with prompt {returned_id:?}"
            )));
        }
        let status = PromptStatus::read(&jstr(run.get("status"), "status", "prompt run")?)?;
        Ok(BoxPromptRun {
            prompt_id: prompt_id.to_string(),
            status,
            done: jbool(run.get("done"), "done", "prompt run")?,
            raw: run,
        })
    }

    /// POST /sandboxes/{id}/steer: an extra instruction for the running turn
    /// of `conversation`, or of the sandbox's most recently active one when
    /// none is named.
    pub async fn steer(
        &self,
        box_id: &str,
        message: &str,
        conversation: Option<&str>,
    ) -> Result<Map<String, Value>, BoxError> {
        if message.is_empty() {
            return Err(BoxError::value(
                "a steer for a Boat sandbox needs a message",
            ));
        }
        let mut body = json!({"message": message});
        if let Some(conversation) = conversation {
            body["conversation"] = Value::from(conversation);
        }
        self.transport
            .request_json(
                "POST",
                &format!("{}/steer", Self::box_path(box_id)?),
                Some(&body),
                &[],
                &["prompt.steered"],
            )
            .await
    }
}
