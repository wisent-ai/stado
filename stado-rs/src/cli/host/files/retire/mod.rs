//! `space file retire` — move one unmanaged executable into its
//! product-owned backup tree.

pub(in crate::cli::host) mod fs;
pub(in crate::cli::host) mod launchd;
pub(in crate::cli::host) mod local;
pub(in crate::cli::host) mod remote;

use crate::cli::CmdError;

/// One exact unmanaged executable, either inspected in place or moved into its
/// product-owned backup tree.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct RetireFileOutcome {
    pub target: String,
    pub source: String,
    pub destination: Option<String>,
    pub transaction: Option<String>,
    pub status: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    pub mode: Option<String>,
    pub detail: Option<String>,
}

impl RetireFileOutcome {
    fn succeeded(&self) -> bool {
        matches!(self.status.as_str(), "ready" | "retired" | "absent")
    }

    fn failure_sentence(&self) -> String {
        format!(
            "{}: {} {}{}",
            self.target,
            self.source,
            self.status,
            self.detail
                .as_ref()
                .map(|detail| format!(" — {detail}"))
                .unwrap_or_default()
        )
    }
}

fn retire_refused(message: impl Into<String>) -> CmdError {
    CmdError::click(format!("space file retire refused: {}", message.into()))
}

#[derive(Debug, Clone, Copy)]
pub struct RetireFileRequest<'a> {
    pub path: &'a str,
    pub product: &'a str,
    pub dry_run: bool,
    pub transaction: Option<&'a str>,
    pub expected_sha256: Option<&'a str>,
    pub expected_size: Option<u64>,
    pub expected_mode: Option<&'a str>,
}

#[derive(Debug)]
struct RetireFileBinding {
    transaction: String,
    expected_sha256: String,
    expected_size: u64,
    expected_mode: String,
}

fn safe_retirement_transaction(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 49
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[8] == b'T'
        && bytes[9..15].iter().all(u8::is_ascii_digit)
        && bytes[15] == b'Z'
        && bytes[16] == b'-'
        && bytes[17..].iter().all(u8::is_ascii_hexdigit)
}

fn retire_file_binding(
    request: &RetireFileRequest<'_>,
) -> Result<Option<RetireFileBinding>, CmdError> {
    match (
        request.transaction,
        request.expected_sha256,
        request.expected_size,
        request.expected_mode,
    ) {
        (None, None, None, None) if request.dry_run => Ok(None),
        (None, None, None, None) => Err(CmdError::usage(
            "mutating space file retire requires transaction, expected-sha256, expected-size, and expected-mode from a reviewed receipt",
        )),
        (Some(transaction), Some(expected_sha256), Some(expected_size), Some(expected_mode)) => {
            if request.dry_run {
                return Err(CmdError::usage(
                    "preflight binding flags are accepted only by the mutating form",
                ));
            }
            if !safe_retirement_transaction(transaction) {
                return Err(CmdError::usage(
                    "transaction must be the exact token from a handoff or space file retire --dry-run receipt",
                ));
            }
            if expected_sha256.len() != 64
                || !expected_sha256.as_bytes().iter().all(u8::is_ascii_hexdigit)
            {
                return Err(CmdError::usage(
                    "expected-sha256 must be a 64-digit hexadecimal SHA-256",
                ));
            }
            if expected_mode.len() != 4
                || !expected_mode
                    .as_bytes()
                    .iter()
                    .all(|byte| matches!(byte, b'0'..=b'7'))
            {
                return Err(CmdError::usage(
                    "expected-mode must be the four-digit octal mode from the handoff or dry-run receipt",
                ));
            }
            Ok(Some(RetireFileBinding {
                transaction: transaction.to_string(),
                expected_sha256: expected_sha256.to_ascii_lowercase(),
                expected_size,
                expected_mode: expected_mode.to_string(),
            }))
        }
        _ => Err(CmdError::usage(
            "transaction, expected-sha256, expected-size, and expected-mode must be supplied together",
        )),
    }
}

fn safe_backup_product(product: &str) -> bool {
    let bytes = product.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
