//! Byte-cursor paging over a job's canonical command log.

use serde_json::{Map, Value};

use crate::machine::{MachineError, MachineFacade};

impl MachineFacade {
    /// Byte-cursor paging over the canonical command log (Python
    /// `read_logs`): `cursor` is a byte offset, `limit` the most bytes the
    /// caller wants (every byte to the end when it names none), `next_cursor`
    /// the offset of the next page, `eof` when the page reaches the current end.
    pub async fn read_logs(
        &self,
        job_id: &str,
        cursor: i64,
        limit: Option<i64>,
    ) -> Result<Value, MachineError> {
        if cursor < 0 {
            return Err(MachineError::new(
                "INVALID_CURSOR",
                "cursor must not be negative",
            ));
        }
        if limit.is_some_and(|limit| limit <= 0) {
            return Err(MachineError::new(
                "INVALID_CURSOR",
                "limit must be positive",
            ));
        }
        self.lookup_job(job_id).await?;
        let payload = match self
            .store
            .read_bytes(&format!("status/{job_id}/output/command_output.log"))
            .await?
        {
            Some(payload) => payload,
            // An absent log of a job its run has reaped is a deleted log, not
            // an empty one; reading it as empty would report a silent job.
            None if self.reaped_job(job_id).await?.is_some() => {
                return Err(MachineError::new(
                    "LOG_REAPED",
                    format!(
                        "the command log of {job_id} was deleted when its run was reaped; the \
                         run manifest retains the job as it ended (`stado machine status \
                         {job_id}`), not its log"
                    ),
                ));
            }
            None => Vec::new(),
        };
        let cursor = cursor as usize;
        if cursor > payload.len() {
            return Err(MachineError::new(
                "INVALID_CURSOR",
                "cursor is beyond the end of the log",
            ));
        }
        let end = limit.map_or(payload.len(), |limit| {
            payload.len().min(cursor + limit as usize)
        });
        let mut out = Map::new();
        out.insert("job_id".into(), Value::from(job_id));
        out.insert("cursor".into(), Value::from(cursor));
        out.insert("next_cursor".into(), Value::from(end));
        out.insert("eof".into(), Value::from(end == payload.len()));
        out.insert(
            "text".into(),
            Value::from(String::from_utf8_lossy(&payload[cursor..end]).into_owned()),
        );
        Ok(Value::Object(out))
    }

    /// A change watch on what a follower of `job_id` reads: the terminal
    /// prefixes and the job's command log directory, armed before the first
    /// read so a follower can hold instead of re-reading on a timer.
    pub fn watch_job(
        &self,
        job_id: &str,
    ) -> Result<Box<dyn crate::queue::ChangeWatch>, MachineError> {
        let log_directory = format!("status/{job_id}/output");
        let mut prefixes: Vec<&str> = crate::queue::runs::TERMINAL_PREFIXES.to_vec();
        prefixes.push(&log_directory);
        Ok(self.store.watch_prefixes(&prefixes)?)
    }
}
