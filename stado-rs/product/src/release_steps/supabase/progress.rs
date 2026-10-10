//! Retained observations from one schema delivery, including partial failure.
//! This document is diagnostic evidence, never admission proof for a release.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

pub(super) struct Progress {
    path: PathBuf,
    document: Value,
}

impl Progress {
    pub(super) fn new(output: &Path, identity: Value) -> Result<Self> {
        let attempt = uuid::Uuid::new_v4();
        let progress = Self {
            path: output.join(format!("supabase-progress-{attempt}.json")),
            document: json!({
                "schema_version": super::super::RECORD_SCHEMA,
                "attempt": attempt.to_string(),
                "identity": identity,
                "state": "in_progress",
                "steps": [],
            }),
        };
        progress.save()?;
        eprintln!("schema delivery progress: {}", progress.path.display());
        Ok(progress)
    }

    fn save(&self) -> Result<()> {
        crate::common::atomic_json(&self.path, &self.document)
            .with_context(|| format!("writing schema delivery progress {}", self.path.display()))
    }

    /// Call only with non-secret observations; credential values are never results.
    pub(super) fn run<T: Serialize>(
        &mut self,
        operation: &str,
        action: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let steps = self.document["steps"]
            .as_array_mut()
            .context("progress steps is not an array")?;
        let index = steps.len();
        steps.push(json!({ "operation": operation, "state": "in_progress" }));
        self.save()?;
        let result = action();
        let step = &mut self.document["steps"][index];
        match &result {
            Ok(observed) => {
                step["state"] = json!("completed");
                step["observed"] = serde_json::to_value(observed).with_context(|| {
                    format!("encoding observed schema delivery operation {operation}")
                })?;
            }
            Err(cause) => {
                step["state"] = json!("failed");
                step["error"] = json!(format!("{cause:#}"));
            }
        }
        match (result, self.save()) {
            (Ok(observed), Ok(())) => Ok(observed),
            (Err(cause), Ok(())) => Err(cause),
            (Ok(_), Err(recording)) => Err(recording.context(format!(
                "schema delivery operation {operation} completed, but its observation could not be retained"
            ))),
            (Err(cause), Err(recording)) => Err(cause.context(format!(
                "retaining failed schema delivery operation {operation} also failed: {recording:#}"
            ))),
        }
    }

    pub(super) fn cleanup(&mut self, path: &Path, result: &std::io::Result<()>) {
        self.document["cleanup"] = match result {
            Ok(()) => json!({ "workspace": path, "state": "completed" }),
            Err(cause) => json!({
                "workspace": path, "state": "failed", "error": cause.to_string(),
            }),
        };
    }

    pub(super) fn finish<T>(&mut self, result: &Result<T>) -> Result<()> {
        self.document["state"] = json!(if result.is_ok() {
            "completed"
        } else {
            "failed"
        });
        if let Err(cause) = result {
            self.document["error"] = json!(format!("{cause:#}"));
        }
        self.save()
    }
}
