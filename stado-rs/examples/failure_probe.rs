//! Before/after probe for the failure vocabulary: everything `failure.rs`
//! derives from a code, dumped so the migration onto `wisent-errors` can be
//! diffed against the local implementation it replaces.
//!
//! Numbers are read from text rather than written as bare literals, the same
//! provenance rule the crate's own exit codes follow.

use stado::failure::{self, FailureCode};

/// The exit code a command already chose, as the probe hands it to
/// `exit_code`: click's runtime code plus one, i.e. the usage code `2`.
fn chosen_exit_code() -> i32 {
    "2".parse().expect("valid chosen exit code")
}

/// The statuses the acceptance criteria name, in the order they name them.
fn probed_statuses() -> Vec<u16> {
    "200 400 401 403 404 407 408 410 429 500 502 503 504 599 600"
        .split_whitespace()
        .map(|digits| digits.parse().expect("valid HTTP status"))
        .collect()
}

fn every_code() -> Vec<FailureCode> {
    vec![
        FailureCode::Config,
        FailureCode::Auth,
        FailureCode::NotFound,
        FailureCode::RateLimit,
        FailureCode::Timeout,
        FailureCode::InfraDown,
        FailureCode::Unknown,
    ]
}

fn main() {
    tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(std::io::stdout)
        .init();

    println!("== per code ==");
    for code in every_code() {
        println!(
            "as_str={} display={code} severity={} retryable={} outage={} exit_code({})={} summary={}",
            code.as_str(),
            code.severity().as_str(),
            code.retryable(),
            code.outage(),
            chosen_exit_code(),
            code.exit_code(chosen_exit_code()),
            code.operator_summary(),
        );
    }

    println!("== retry exit code ==");
    println!("retry_exit_code={}", failure::retry_exit_code());

    println!("== from_upstream_status, named statuses ==");
    for status in probed_statuses() {
        println!(
            "{status} -> {}",
            FailureCode::from_upstream_status(status).as_str()
        );
    }

    println!("== from_upstream_status, every u16, as runs ==");
    let mut run_start = u16::MIN;
    let mut run_code = FailureCode::from_upstream_status(u16::MIN);
    for status in u16::MIN..=u16::MAX {
        let code = FailureCode::from_upstream_status(status);
        if code != run_code {
            println!("{run_start}..={} -> {}", status - 1, run_code.as_str());
            run_start = status;
            run_code = code;
        }
    }
    println!("{run_start}..={} -> {}", u16::MAX, run_code.as_str());

    println!("== operator_line ==");
    for code in every_code() {
        println!("{}", failure::operator_line(code));
    }

    println!("== classify_message ==");
    let messages = [
        "GCS API error HTTP 503: backend not found",
        "list queue/ -> HTTP 500 Internal Server Error: no such object",
        "Stado object API returned HTTP 404 Not Found",
        "HTTP 429: slow down",
        "HTTP/1.1 401 Unauthorized",
        "HTTP 504: upstream",
        "HTTP 418: connection refused",
        "WC_BUCKET is required",
        "gcloud: command not found",
        "blob not found: queue/1a2b3c4d.json",
        "authentication failed for service account",
        "quota exceeded, retry after 30s",
        "operation timed out after deadline exceeded",
        "error sending request: tcp connect error",
        "unknown job 1a2b3c4d",
        "the disk fell over in a way nobody wrote a needle for",
    ];
    for message in messages {
        println!(
            "{} <- {message}",
            failure::classify_message(message).as_str()
        );
    }

    println!("== bounded_detail ==");
    let long = "x".repeat("4096".parse().expect("valid probe length"));
    println!("bounded_detail_len={}", failure::bounded_detail(&long).len());
    println!(
        "bounded_detail_trims={:?}",
        failure::bounded_detail("   an upstream said no   ")
    );

    println!("== full rendered failure ==");
    let message = "GCS API error HTTP 503: could not read queue/1a2b3c4d.json";
    let code = failure::classify_message(message);
    println!("Error: {message}");
    println!("{}", failure::operator_line(code));
    failure::log_failure("cli.storage.ls", "queue", code, message);
}
