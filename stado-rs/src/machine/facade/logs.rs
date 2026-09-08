//! Byte-cursor paging over a job's canonical command log.

use serde_json::{Map, Value};

use crate::machine::{MachineError, MachineFacade};

impl MachineFacade {
    /// Byte-cursor paging over the canonical command log (Python
    /// `read_logs`): `cursor` is a byte offset, `next_cursor` the offset of
    /// the next page, `eof` when the page reaches the current end.
    pub async fn read_logs(
        &self,
        job_id: &str,
        cursor: i64,
        limit: i64,
    ) -> Result<Value, MachineError> {
        if cursor < 0 {
            return Err(MachineError::new(
                "INVALID_CURSOR",
                "cursor must not be negative",
            ));
        }
        if limit <= 0 {
            return Err(MachineError::new(
                "INVALID_CURSOR",
                "limit must be positive",
            ));
        }
        self.lookup_job(job_id).await?;
        let payload = self
            .store
            .read_bytes(&format!("status/{job_id}/output/command_output.log"))
            .await?
            .unwrap_or_default();
        let cursor = cursor as usize;
        if cursor > payload.len() {
            return Err(MachineError::new(
                "INVALID_CURSOR",
                "cursor is beyond the end of the log",
            ));
        }
        let end = payload.len().min(cursor + limit as usize);
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
}
