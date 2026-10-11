//! What the operating system and the HTTP transport say about a failure: the
//! class an I/O kind or an HTTP error states, the host's clock, the macOS
//! privacy refusal, and the whole cause chain of an HTTP failure.

use super::CmdError;

/// The failure class an operating-system error states by its kind, read from
/// the kind the kernel returned and never from the message. A path the host
/// would not open is not a credential of ours being rejected: telling the
/// operator to check credentials sends them to the vault for a file the
/// operating system refused, so it stays unclassified with its detail intact,
/// as the message classifier's `filesystem_refusal` family does.
pub fn io_failure_code(kind: std::io::ErrorKind) -> crate::primitives::failure::FailureCode {
    use crate::primitives::failure::FailureCode;
    use std::io::ErrorKind;
    match kind {
        ErrorKind::NotFound => FailureCode::NotFound,
        ErrorKind::PermissionDenied => FailureCode::Unknown,
        ErrorKind::TimedOut => FailureCode::Timeout,
        ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::NotConnected
        | ErrorKind::AddrNotAvailable
        | ErrorKind::BrokenPipe => FailureCode::InfraDown,
        ErrorKind::InvalidInput | ErrorKind::InvalidData | ErrorKind::AlreadyExists => {
            FailureCode::Refused
        }
        _ => FailureCode::Unknown,
    }
}

/// Seconds since the Unix epoch on this host's clock. A clock that reads
/// before the epoch is this host's configuration, so it is refused as config
/// naming the clock rather than passed on as a bare system-time error.
pub fn epoch_seconds() -> Result<u64, CmdError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .map_err(|error| {
            CmdError::click(format!(
                "this host's clock reads before the Unix epoch, so no expiry can be compared: {error}"
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })
}

/// What an `EPERM` from the kernel means for the program that received it.
/// On macOS a file this account owns answers `Operation not permitted` when
/// the system's privacy controls withhold its folder (Documents, Desktop,
/// Downloads, removable volumes) from the program asking, which is decided
/// per executable and survives restarts, so the operator has to know which
/// executable to allow. `EACCES` is ordinary file permission and says enough
/// by itself.
pub fn operating_system_refusal(error: &std::io::Error) -> Option<String> {
    if !cfg!(target_os = "macos") || error.raw_os_error() != Some(nix::libc::EPERM) {
        return None;
    }
    let program = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|exe_error| format!("this program (its path is unreadable: {exe_error})"));
    Some(format!(
        "macOS refused {program} with EPERM, which is how its privacy controls refuse a \
         program a protected folder: allow {program} in System Settings → Privacy & \
         Security → Files and Folders (or Full Disk Access)"
    ))
}

/// The whole cause chain of one HTTP failure, joined, with the URL it was
/// asking for.
///
/// `reqwest::Error`'s own `Display` is frequently one unattributable word, and
/// `builder error` is the worst of them: it names no URL, no header and no
/// field. It can be the only thing `stado storage stat` says for one
/// product's catalog object while the same command for other products
/// answers an honest HTTP 401 — so the operator's only signal that the
/// fault is in a credential rather than in the network is that one product
/// differs from the others. The answer is
/// one layer down, in a source chain nothing printed: a header value that
/// could not be built. Every reqwest failure that reaches an operator now
/// carries that chain, because the layer that knows the cause is never the one
/// whose message gets shown.
pub fn http_failure(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut cause: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(error);
    while let Some(current) = cause {
        let text = current.to_string();
        if !text.is_empty() && !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        cause = current.source();
    }
    if let Some(url) = error.url() {
        message.push_str(&format!(" (requesting {url})"));
    }
    message
}

/// The failure code an HTTP error states about itself.
///
/// reqwest marks a refused connection (`is_connect`) and an elapsed deadline
/// (`is_timeout`), and nothing else: a body the peer stopped reading mid-way
/// (`connection reset by peer`, `broken pipe`, an unexpected end of stream)
/// reaches it as a request or body error whose operating-system cause is in
/// the source chain. Such an error carried no code, so the operator line read
/// "your request or credentials [unknown]" about a transfer the network
/// ended, and the wrapper could not retry it. The chain is read for that
/// cause: an I/O failure anywhere in it is the transport's, so the error is
/// an outage worth retrying.
pub fn http_failure_code(
    error: &reqwest::Error,
) -> Option<crate::primitives::failure::FailureCode> {
    use crate::primitives::failure::FailureCode;
    if error.is_timeout() {
        return Some(FailureCode::Timeout);
    }
    if error.is_connect() {
        return Some(FailureCode::InfraDown);
    }
    let mut cause: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(error);
    while let Some(current) = cause {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            return Some(match io.kind() {
                std::io::ErrorKind::TimedOut => FailureCode::Timeout,
                _ => FailureCode::InfraDown,
            });
        }
        cause = current.source();
    }
    // A connection the peer closed cleanly before answering carries no
    // operating-system cause, only hyper's own sentence; it is still the
    // transport ending the request or its body, never the caller's request.
    if error.is_request() || error.is_body() {
        return Some(FailureCode::InfraDown);
    }
    None
}

impl From<reqwest::Error> for CmdError {
    fn from(exc: reqwest::Error) -> Self {
        Self {
            failure: http_failure_code(&exc),
            ..Self::click(http_failure(&exc))
        }
    }
}

impl From<crate::providers::ProviderError> for CmdError {
    fn from(exc: crate::providers::ProviderError) -> Self {
        match exc {
            error @ (crate::providers::ProviderError::Disabled(_)
            | crate::providers::ProviderError::NotEnabled(_)) => Self::refused(error.to_string()),
            error => Self::click(error.to_string()),
        }
    }
}
