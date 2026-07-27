//! Bounded command, prompt, and artifact output helpers.
//!
//! Port of `stado/scheduler/dispatch/box/output.py`, plus the two helpers
//! it imports from `stado/providers/local/helpers/execution.py`
//! (`build_job_command`, `verify_command` — "Pure shell command assembly
//! shared by local and structured providers").
//!
//! Deviation: Python `upload_artifacts` requires an SDK storage backend
//! (`store._blob_backend` / `store._sdk_bucket`) and raises RuntimeError
//! otherwise; every Rust `BlobBackend` uploads bytes by contract, so the
//! gate has no analog and artifacts always land via
//! [`crate::queue::JobStorage::upload_bytes`].

use serde_json::{Map, Value};

use crate::models::Job;
use crate::providers::r#box::{BoxClient, BoxError};

use super::runtime::Keepalive;
use super::BoxDispatchError;

pub const LOG_BYTES: usize = 57344;
pub const ARTIFACT_BYTES: usize = 16777216;
pub const ARTIFACT_COUNT: usize = 16;
pub const EVENT_PAGES: usize = 10;
pub const EVENT_LIMIT: i64 = 100;

/// Python `runtime_paths(job_id)` dict.
#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub root: String,
    pub script: String,
    pub stdout: String,
    pub stderr: String,
    pub exit: String,
    pub pid: String,
    pub launch: String,
}

/// Python `runtime_paths`.
pub fn runtime_paths(job_id: &str) -> RuntimePaths {
    let root = format!(".stado/{job_id}");
    RuntimePaths {
        script: format!("{root}/run.sh"),
        stdout: format!("{root}/stdout.log"),
        stderr: format!("{root}/stderr.log"),
        exit: format!("{root}/exit_code"),
        pid: format!("{root}/pid"),
        launch: format!("{root}/launch_intent"),
        root,
    }
}

/// Python `shlex.quote`: return the string unchanged when it matches
/// `[^\w@%+=:,./-]` nowhere (re.ASCII word chars), else single-quote with
/// the `'"'"'` escape.
pub(crate) fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | '@' | '%' | '+' | '=' | ',' | ':' | '.' | '/' | '-')
        });
    if safe {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// Python `repo_prelude`.
fn repo_prelude(job: &Job) -> String {
    let repo = job.repo.trim();
    if repo.is_empty() {
        return String::new();
    }
    let mut workdir = job.repo_workdir.trim().to_string();
    if workdir.is_empty() {
        workdir = repo
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .strip_suffix(".git")
            .unwrap_or(repo.trim_end_matches('/').rsplit('/').next().unwrap_or(""))
            .to_string();
    }
    let extras = job.repo_extras.trim();
    let install = if extras.is_empty() {
        String::new()
    } else {
        format!(
            " && pip install --break-system-packages --upgrade pip setuptools wheel \
             && pip install --break-system-packages --no-build-isolation '.[{extras}]'"
        )
    };
    format!("rm -rf {workdir} && git clone --depth 1 {repo} {workdir} && cd {workdir}{install} && cd .. && ")
}

/// Python `pre_command_prelude`.
fn pre_command_prelude(job: &Job) -> String {
    let pre = job.pre_command.trim();
    if pre.is_empty() {
        return String::new();
    }
    format!("{} && ", pre.trim_end_matches(';').trim_end())
}

/// Python `build_job_command`.
pub fn build_job_command(job: &Job) -> String {
    format!(
        "{}{}{}",
        repo_prelude(job),
        pre_command_prelude(job),
        job.command
    )
}

/// Python `verify_command`.
pub fn verify_command(job: &Job) -> String {
    job.verify_command.trim().to_string()
}

/// RFC 4648 standard-alphabet base64 with padding (Python `base64.b64encode`).
/// Hand-rolled because the crate's dependency set has no base64 crate.
fn base64_encode(data: &[u8]) -> String {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Python `command_wrapper`: the idempotent run.sh written into the box.
pub fn command_wrapper(job: &Job, paths: &RuntimePaths) -> String {
    let command = shell_quote(&build_job_command(job));
    let verification = verify_command(job);
    let stdout = shell_quote(&paths.stdout);
    let stderr = shell_quote(&paths.stderr);
    let artifact_file = format!("{}/artifacts.json", paths.root);
    // json.dumps(..., sort_keys=True, separators=(",", ":")) then b64.
    let sorted: std::collections::BTreeMap<String, Value> =
        job.resolved_input_artifacts.clone().into_iter().collect();
    let artifact_payload = base64_encode(
        serde_json::to_string(&sorted)
            .expect("artifact map serialization is infallible")
            .as_bytes(),
    );
    let mut lines = vec![
        "#!/bin/bash".to_string(),
        "set +e".to_string(),
        "umask 077".to_string(),
        format!("mkdir -p {}", shell_quote(&paths.root)),
        format!(
            "printf '%s' {} | base64 --decode > {}",
            shell_quote(&artifact_payload),
            shell_quote(&artifact_file)
        ),
        format!("export WC_ARTIFACT_INPUTS_FILE={}", shell_quote(&artifact_file)),
        format!("export WC_ARTIFACT_INPUTS_JSON=\"$(cat {})\"", shell_quote(&artifact_file)),
        format!(
            "bash -lc {command} > >(tail -c {LOG_BYTES} >{stdout}) 2> >(tail -c {LOG_BYTES} >{stderr})"
        ),
        "rc=$?".to_string(),
        "wait".to_string(),
    ];
    if !verification.is_empty() {
        lines.extend([
            "if [ \"$rc\" -eq 0 ]; then".to_string(),
            format!(
                "  bash -lc {} > >(tail -c {LOG_BYTES} >>{stdout}) 2> >(tail -c {LOG_BYTES} >>{stderr})",
                shell_quote(&verification)
            ),
            "  rc=$?".to_string(),
            "  wait".to_string(),
            "fi".to_string(),
        ]);
    }
    for path in [&paths.stdout, &paths.stderr] {
        let path = shell_quote(path);
        lines.push(format!(
            "test ! -f {path} || (tail -c {LOG_BYTES} {path} >{path}.tmp && mv {path}.tmp {path})"
        ));
    }
    lines.push(format!("printf '%s' \"$rc\" >{}", shell_quote(&paths.exit)));
    lines.push("exit \"$rc\"".to_string());
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Python `file_content`: unwrap the nested `{"file": {...}}` envelope and
/// require string content.
pub fn file_content(value: &Map<String, Value>) -> Result<String, BoxError> {
    let nested = match value.get("file") {
        Some(Value::Object(file)) => file,
        _ => value,
    };
    match nested.get("content") {
        Some(Value::String(content)) => Ok(content.clone()),
        _ => Err(BoxError::transport(
            "Box file response omitted string content",
        )),
    }
}

/// Python `recover_prompt_id`: scan prompt events for the operation marker.
pub(crate) async fn recover_prompt_id(
    client: &BoxClient,
    box_id: &str,
    marker: &str,
    keepalive: &mut Keepalive<'_, '_>,
) -> Result<String, BoxDispatchError> {
    let mut cursor = String::new();
    for _ in 0..EVENT_PAGES {
        let page = client
            .list_events(box_id, &cursor, EVENT_LIMIT, "asc", "prompt")
            .await?;
        keepalive.ping().await?;
        for event in &page.events {
            let empty = Map::new();
            let data = match event.get("data") {
                Some(Value::Object(data)) => data,
                _ => &empty,
            };
            let prompt = data.get("prompt").and_then(Value::as_str).unwrap_or("");
            if event.get("type").and_then(Value::as_str) == Some("prompt")
                && prompt.starts_with(marker)
            {
                let id = event
                    .get("taskId")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .or_else(|| event.get("id").and_then(Value::as_str))
                    .unwrap_or("");
                return Ok(id.to_string());
            }
        }
        if !page.has_more || page.next_cursor.is_empty() {
            break;
        }
        cursor = page.next_cursor;
    }
    Ok(String::new())
}

/// Python `prompt_output`: join response-event contents bounded to
/// LOG_BYTES (byte-exact truncation with lossy UTF-8 decode).
pub(crate) async fn prompt_output(
    client: &BoxClient,
    box_id: &str,
    prompt_id: &str,
    keepalive: &mut Keepalive<'_, '_>,
) -> Result<String, BoxDispatchError> {
    let mut cursor = String::new();
    let mut parts: Vec<String> = Vec::new();
    let mut size = 0usize;
    for _ in 0..EVENT_PAGES {
        let page = client
            .list_events(box_id, &cursor, EVENT_LIMIT, "asc", "response")
            .await?;
        keepalive.ping().await?;
        for event in &page.events {
            let empty = Map::new();
            let data = match event.get("data") {
                Some(Value::Object(data)) => data,
                _ => &empty,
            };
            let Some(content) = data.get("content").and_then(Value::as_str) else {
                continue;
            };
            if event.get("type").and_then(Value::as_str) != Some("response")
                || event.get("taskId").and_then(Value::as_str) != Some(prompt_id)
                || data
                    .get("is_streaming")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                || data
                    .get("is_reverted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                continue;
            }
            let encoded = content.as_bytes();
            let remaining = LOG_BYTES - size;
            if remaining == 0 {
                return Ok(parts.join("\n"));
            }
            let take = encoded.len().min(remaining);
            parts.push(String::from_utf8_lossy(&encoded[..take]).into_owned());
            size += take;
        }
        if !page.has_more || page.next_cursor.is_empty() {
            break;
        }
        cursor = page.next_cursor;
    }
    Ok(parts.join("\n"))
}

/// Python `_safe_artifact_path` (ValueError).
fn safe_artifact_path(value: &str) -> Result<String, BoxDispatchError> {
    let path = value.trim();
    // PurePosixPath semantics: absolute paths and any ".." part are
    // rejected; repeated slashes / "." parts normalize away.
    let bad = path.is_empty() || path.starts_with('/') || path.split('/').any(|part| part == "..");
    if bad {
        return Err(BoxDispatchError::value(
            "Box artifact path must be relative and contained",
        ));
    }
    Ok(path.to_string())
}

/// Python `upload_artifacts`: bounded artifact collection into status/.
pub(crate) async fn upload_artifacts(
    store: &crate::queue::JobStorage,
    client: &BoxClient,
    job: &Job,
    box_id: &str,
    keepalive: &mut Keepalive<'_, '_>,
) -> Result<(), BoxDispatchError> {
    if job.artifact_paths.len() > ARTIFACT_COUNT {
        return Err(BoxDispatchError::value("too many Box artifacts requested"));
    }
    let mut remaining = ARTIFACT_BYTES;
    for source in &job.artifact_paths {
        keepalive.ping().await?;
        let path = safe_artifact_path(source)?;
        if remaining == 0 {
            return Err(BoxDispatchError::value(
                "Box artifact aggregate byte bound exceeded",
            ));
        }
        let content = client.download_artifact(box_id, &path, remaining).await?;
        remaining -= content.len();
        let destination = format!(
            "status/{}/output/artifacts/{}",
            job.job_id,
            path.replace('/', "_")
        );
        store.upload_bytes(&destination, &content).await?;
        keepalive.ping().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_matches_shlex() {
        assert_eq!(
            shell_quote("abc-DEF_123/@%+=:,./-"),
            "abc-DEF_123/@%+=:,./-"
        );
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\"'\"'s'");
        // Unicode is unsafe under re.ASCII \w.
        assert_eq!(shell_quote("ż"), "'ż'");
    }

    #[test]
    fn base64_encode_matches_python() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"{\"a\":1}"), "eyJhIjoxfQ==");
    }

    #[test]
    fn runtime_paths_layout() {
        let paths = runtime_paths("job-1");
        assert_eq!(paths.root, ".stado/job-1");
        assert_eq!(paths.script, ".stado/job-1/run.sh");
        assert_eq!(paths.exit, ".stado/job-1/exit_code");
        assert_eq!(paths.launch, ".stado/job-1/launch_intent");
    }

    #[test]
    fn build_job_command_assembles_repo_pre_and_command() {
        let mut job = Job::new("j", "python run.py");
        assert_eq!(build_job_command(&job), "python run.py");
        job.repo = "https://github.com/org/repo.git".into();
        job.repo_extras = "train".into();
        job.pre_command = "export A=1;".into();
        let cmd = build_job_command(&job);
        assert_eq!(
            cmd,
            "rm -rf repo && git clone --depth 1 https://github.com/org/repo.git repo \
             && cd repo && pip install --break-system-packages --upgrade pip setuptools wheel \
             && pip install --break-system-packages --no-build-isolation '.[train]' \
             && cd .. && export A=1 && python run.py"
        );
        // Empty extras skips the install stanza; repo_workdir wins.
        job.repo_extras = String::new();
        job.repo_workdir = "custom-dir".into();
        let cmd = build_job_command(&job);
        assert!(
            cmd.starts_with("rm -rf custom-dir && git clone --depth 1"),
            "{cmd}"
        );
        assert!(!cmd.contains("pip install"), "{cmd}");
    }

    #[test]
    fn command_wrapper_bounds_logs_and_writes_exit() {
        let mut job = Job::new("j9", "echo 'hi there'");
        job.verify_command = "test -f out".into();
        job.resolved_input_artifacts
            .insert("b".into(), Value::from(1));
        job.resolved_input_artifacts
            .insert("a".into(), Value::from(2));
        let script = command_wrapper(&job, &runtime_paths("j9"));
        assert!(
            script.starts_with("#!/bin/bash\nset +e\numask 077\n"),
            "{script}"
        );
        // sorted compact JSON: {"a":2,"b":1} -> eyJhIjoyLCJiIjoxfQ==
        assert!(script.contains("eyJhIjoyLCJiIjoxfQ=="), "{script}");
        assert!(
            script.contains("bash -lc 'echo '\"'\"'hi there'\"'\"''"),
            "{script}"
        );
        assert!(script.contains("tail -c 57344"), "{script}");
        assert!(script.contains("bash -lc 'test -f out'"), "{script}");
        assert!(
            script.contains("printf '%s' \"$rc\" >.stado/j9/exit_code"),
            "{script}"
        );
        assert!(script.ends_with("exit \"$rc\"\n"), "{script}");
    }

    #[test]
    fn file_content_unwraps_envelope_and_requires_string() {
        let flat = Map::from_iter([("content".to_string(), Value::from("data"))]);
        assert_eq!(file_content(&flat).unwrap(), "data");
        let nested = Map::from_iter([(
            "file".to_string(),
            Value::Object(Map::from_iter([(
                "content".to_string(),
                Value::from("deep"),
            )])),
        )]);
        assert_eq!(file_content(&nested).unwrap(), "deep");
        let bad = Map::from_iter([("content".to_string(), Value::from(3))]);
        let err = file_content(&bad).unwrap_err();
        assert_eq!(err.to_string(), "Box file response omitted string content");
    }

    #[test]
    fn safe_artifact_path_rejects_absolute_and_traversal() {
        assert!(safe_artifact_path("out/result.json").is_ok());
        assert!(safe_artifact_path("a//b").is_ok());
        for bad in ["/abs", "../up", "a/../../b", "  "] {
            let err = safe_artifact_path(bad).unwrap_err();
            assert!(err.to_string().contains("relative and contained"), "{err}");
        }
    }
}
