//! The digest contract: whether the staged archive is the one the host agreed
//! to run, and reading the one field of a host's `shasum` line that says so.

use super::*;

/// Whether a staged archive is the one the coordinate declares.
///
/// The digest is the whole contract between what was delivered and what the
/// host agreed to run, so a mismatch refuses rather than installs.
pub fn digest_verdict(declared: &str, observed: &str) -> Result<(), DeployError> {
    if declared.eq_ignore_ascii_case(observed) {
        return Ok(());
    }
    Err(DeployError(format!(
        "the staged archive hashes to {observed}, but the deployment env file declares \
         {declared}; refusing to activate an archive the host has not agreed to run - stage \
         the declared archive or update the deployment env declaration"
    )))
}

/// The first hex field of `shasum -a 256`.
pub fn parse_shasum(stdout: &str) -> Option<&str> {
    stdout
        .split_whitespace()
        .next()
        .filter(|field| field.len() == 64 && field.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_archive_that_is_not_the_declared_one_refuses_before_anything_runs() {
        let declared = "a".repeat(64);
        let observed = "b".repeat(64);
        let said = digest_verdict(&declared, &observed)
            .unwrap_err()
            .to_string();
        assert!(said.contains("has not agreed to run"), "{said}");
        // Case is the only thing a host's tooling is allowed to differ on.
        digest_verdict(&declared, &declared.to_uppercase()).unwrap();
    }

    #[test]
    fn a_shasum_line_yields_only_a_real_digest() {
        assert_eq!(
            parse_shasum(
                "2714720eea1eaa430000000000000000000000000000000000000000000000ab  /path\n"
            ),
            Some("2714720eea1eaa430000000000000000000000000000000000000000000000ab")
        );
        assert_eq!(parse_shasum("shasum: no such file\n"), None);
        assert_eq!(parse_shasum(""), None);
    }
}
