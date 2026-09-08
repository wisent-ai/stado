//! Preparing a transition: deriving the destination document from a fresh
//! source read and installing the durable record that recovery replays.

use chrono::Utc;

use crate::models::Job;
use crate::queue::storage::records::{
    merge_transition_destination, prefix_state, sha256_hex, JobTransition,
    TRANSITION_CLEANED_PREFIX, TRANSITION_SCHEMA,
};
use crate::queue::storage::{transition_path, JobStorage};
use crate::queue::StorageError;

impl JobStorage {
    pub(in crate::queue::storage::lifecycle) async fn transition_job_if_version(
        &self,
        requested: &Job,
        from_prefix: &str,
        to_prefix: &str,
        expected_version: Option<&str>,
    ) -> Result<bool, StorageError> {
        for _ in 0..16 {
            self.recover_job_transition(&requested.job_id).await?;
            let source_path = format!("{from_prefix}/{}.json", requested.job_id);
            let Some(source_versioned) = self.read_text_versioned(&source_path).await? else {
                if let Some(existing) = self.read_job(to_prefix, &requested.job_id).await? {
                    if crate::queue::submit::immutable_job_projection(&existing)
                        == crate::queue::submit::immutable_job_projection(requested)
                    {
                        return Ok(true);
                    }
                }
                return Ok(false);
            };
            if expected_version.is_some_and(|expected| expected != source_versioned.version) {
                return Ok(false);
            }
            let current = Job::from_json(&source_versioned.content)?;
            if current.state != prefix_state(from_prefix) {
                self.recover_job_transition(&requested.job_id).await?;
                return Ok(false);
            }
            let destination_versioned = self
                .read_text_versioned(&format!("{to_prefix}/{}.json", requested.job_id))
                .await?;
            let (destination_basis, destination_version) = match destination_versioned {
                Some(existing_versioned) => {
                    let existing = Job::from_json(&existing_versioned.content)?;
                    if crate::queue::submit::immutable_job_projection(&existing)
                        != crate::queue::submit::immutable_job_projection(&current)
                    {
                        return Err(StorageError::StorageConflict(format!(
                            "{to_prefix}/{}.json belongs to different lifecycle data",
                            requested.job_id
                        )));
                    }
                    if existing.state.starts_with(TRANSITION_CLEANED_PREFIX) {
                        (requested.clone(), Some(existing_versioned.version))
                    } else if existing.state == prefix_state(to_prefix) {
                        (existing, Some(existing_versioned.version))
                    } else {
                        return Err(StorageError::StorageConflict(format!(
                            "{to_prefix}/{}.json is not reusable for lifecycle transition",
                            requested.job_id
                        )));
                    }
                }
                None => (requested.clone(), None),
            };
            let destination =
                merge_transition_destination(&current, &destination_basis, from_prefix, to_prefix)?;
            let transition_id = sha256_hex(
                format!(
                    "{}\0{}\0{}\0{}\0{}",
                    requested.job_id,
                    from_prefix,
                    to_prefix,
                    source_versioned.version,
                    destination.to_json()
                )
                .as_bytes(),
            );
            let candidate = JobTransition {
                schema: TRANSITION_SCHEMA.to_string(),
                transition_id,
                owner: uuid::Uuid::new_v4().simple().to_string(),
                job_id: requested.job_id.clone(),
                from_prefix: from_prefix.to_string(),
                to_prefix: to_prefix.to_string(),
                source_version: source_versioned.version.clone(),
                source_digest: sha256_hex(source_versioned.content.as_bytes()),
                destination_version,
                state: "prepared".into(),
                created_at: Utc::now().to_rfc3339(),
                destination_job: destination,
            };
            let path = transition_path(&requested.job_id);
            let body = serde_json::to_string_pretty(&candidate)?;
            let installed = match self.read_text_versioned(&path).await? {
                None => self.create_text_if_absent(&path, &body).await?,
                Some(active) => {
                    let existing: JobTransition = serde_json::from_str(&active.content)?;
                    if existing.state == "prepared" {
                        self.recover_job_transition(&requested.job_id).await?;
                        continue;
                    }
                    match self
                        .compare_and_swap_text(&path, &active.version, &body)
                        .await
                    {
                        Ok(_) => true,
                        Err(StorageError::StorageConflict(_)) => false,
                        Err(error) => return Err(error),
                    }
                }
            };
            if !installed {
                continue;
            }
            if self.recover_job_transition(&requested.job_id).await? {
                return Ok(true);
            }
        }
        Err(StorageError::StorageConflict(format!(
            "job {} remained contended during lifecycle transition",
            requested.job_id
        )))
    }
}
