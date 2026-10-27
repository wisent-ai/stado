//! Validated submission entry points for configured and explicitly selected stores.

use crate::config;
use crate::models::Job;
use crate::queue::storage::JobStorage;

use super::{validate_run_id, validate_submission, SubmitError, SubmitOptions};

mod store;

fn validate(commands: &[String], options: &SubmitOptions) -> Result<(), SubmitError> {
    if commands.is_empty() {
        return Err(SubmitError::Validation(
            "at least one command is required".into(),
        ));
    }
    validate_run_id(&options.run_id)?;
    for command in commands {
        validate_submission(command, options)?;
    }
    Ok(())
}

/// Persist a complete immutable plan before any configured-store queue write.
pub async fn submit_batch(
    commands: &[String],
    options: &SubmitOptions,
) -> Result<Vec<Job>, SubmitError> {
    validate(commands, options)?;
    let bucket = if options.bucket.is_empty() {
        config::bucket()
    } else {
        options.bucket.as_str()
    };
    let selected = JobStorage::with_bucket(bucket).await?;
    store::submit(&selected, commands, options).await
}

impl JobStorage {
    /// Submit and replay on this store without opening the configured fleet store.
    /// The request must explicitly name this store's bucket.
    pub async fn submit_batch(
        &self,
        commands: &[String],
        options: &SubmitOptions,
    ) -> Result<Vec<Job>, SubmitError> {
        validate(commands, options)?;
        if options.bucket != self.bucket_name() {
            return Err(SubmitError::Validation(format!(
                "submission bucket {:?} does not match selected store bucket {:?}",
                options.bucket,
                self.bucket_name()
            )));
        }
        store::submit(self, commands, options).await
    }
}
