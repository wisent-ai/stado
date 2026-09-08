//! Box prompt submission and prompt-run status payload readers.
//!
//! Port of the prompt half of `stado/providers/box/client.py`.

use serde_json::{json, Value};

use super::super::types::{jbool, jstr, required_dict, BoxError, BoxPromptRun};
use super::BoxClient;

impl BoxClient {
    /// POST /boxes/{id}/prompt. The response must carry a `promptRun` dict
    /// whose id matches the top-level `promptId` (when present).
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
        let prompt_run = required_dict(
            value.get("promptRun").cloned().unwrap_or(Value::Null),
            "prompt",
        )
        .map_err(|_| BoxError::transport("Box prompt response omitted promptRun"))?;
        let prompt_id = super::super::types::first_truthy_str(
            &[value.get("promptId"), prompt_run.get("promptId")],
            "",
        );
        if prompt_id.is_empty() || jstr(prompt_run.get("promptId")) != prompt_id {
            return Err(BoxError::transport(
                "Box prompt response has an invalid prompt id",
            ));
        }
        Ok(BoxPromptRun {
            prompt_id,
            status: {
                let status = jstr(prompt_run.get("status"));
                if status.is_empty() {
                    "queued".to_string()
                } else {
                    status
                }
            },
            done: jbool(prompt_run.get("done")),
            raw: prompt_run,
        })
    }

    /// GET /boxes/{id}/prompts/{prompt_id}.
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
        let prompt_run = required_dict(
            value.get("promptRun").cloned().unwrap_or(Value::Null),
            "prompt",
        )
        .map_err(|_| BoxError::transport("Box prompt status omitted promptRun"))?;
        let returned_id = jstr(prompt_run.get("promptId"));
        if returned_id != prompt_id {
            return Err(BoxError::transport("Box prompt status id mismatch"));
        }
        let status = jstr(prompt_run.get("status"));
        if !["sending", "queued", "running", "finished", "failed"].contains(&status.as_str()) {
            return Err(BoxError::transport("Box prompt status is invalid"));
        }
        Ok(BoxPromptRun {
            prompt_id: prompt_id.to_string(),
            status,
            done: jbool(prompt_run.get("done")),
            raw: prompt_run,
        })
    }
}
