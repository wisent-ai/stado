//! The proof that an S3 API endpoint other than Amazon's honours the two
//! preconditions Stado's writes rely on.
//!
//! The queue's claims, leases and release publications are if-absent creates
//! and compare-and-swap replacements. An endpoint that accepts the S3 API but
//! ignores `If-None-Match` or `If-Match` does not refuse them: it stores the
//! write and answers 200, so a lost race reads as a won one and two holders
//! each believe they own the same lease. Nothing about the endpoint says which
//! kind it is, and providers add the headers release by release, so the store
//! is asked: one object under a name no one else can hold is created, created
//! again, and replaced against a version it cannot carry. The second create
//! and the replacement must both be refused with 412. A verdict is kept for
//! the backend's life and read before every write that relies on it; a check
//! that could not run (the store unreachable, a request refused for another
//! reason, the proof object left behind) decides nothing, and the next such
//! write asks again.

use aws_sdk_s3::primitives::ByteStream;

use crate::queue::StorageError;

use super::super::{
    refusals::{is_precondition_failed, sdk_err},
    Conditional, S3Backend,
};

/// Where the proof's object lives: beside the store's layout marker, in the
/// part of the canonical layout Stado itself owns.
const PROBE_PREFIX: &str = "system/conditional-writes/";
/// Random bytes in the proof object's name and in the version it is replaced
/// against: the size of an MD5 digest, which is what a single-part S3 ETag
/// carries, so the version is shaped like one the store could hold.
const NONCE_BYTES: usize = 16;

fn nonce() -> Result<String, String> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; NONCE_BYTES];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "system randomness is unavailable".to_string())?;
    Ok(hex::encode(bytes))
}

impl S3Backend {
    /// Refuse a write that relies on a precondition when this store has not
    /// shown it honours them.
    pub(super) async fn require_conditional_writes(&self) -> Result<(), StorageError> {
        match &self.inner.conditional {
            Conditional::Documented => Ok(()),
            Conditional::Proved { store, outcome } => {
                match outcome
                    .get_or_try_init(|| self.prove_conditional_writes(store))
                    .await
                {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(verdict)) => Err(StorageError::Other(verdict.clone())),
                    Err(undecided) => Err(StorageError::Other(undecided)),
                }
            }
        }
    }

    /// `Ok(verdict)` when the store answered every step, the verdict being
    /// whether it honours both preconditions; `Err` when the check could not
    /// be completed.
    async fn prove_conditional_writes(&self, store: &str) -> Result<Result<(), String>, String> {
        let key = format!("{PROBE_PREFIX}{}", nonce()?);
        let create = || {
            crate::wait::sdk(
                self.inner
                    .client
                    .put_object()
                    .bucket(&self.inner.bucket)
                    .key(&key)
                    .if_none_match("*")
                    .body(ByteStream::from_static(b"conditional-write proof"))
                    .send(),
            )
        };
        if let Err(error) = create().await {
            return Err(format!(
                "{store} could not be checked for conditional writes: creating {key} failed: {}",
                sdk_err("put_object if-absent", error)
            ));
        }
        let second = create().await;
        let wrong_version = nonce()?;
        let replace = crate::wait::sdk(
            self.inner
                .client
                .put_object()
                .bucket(&self.inner.bucket)
                .key(&key)
                .if_match(format!("\"{wrong_version}\""))
                .body(ByteStream::from_static(b"conditional-write proof replaced"))
                .send(),
        )
        .await;
        // Removed before the answer is judged, so every answer can say
        // whether the proof object is still there.
        let left = match crate::wait::sdk(
            self.inner
                .client
                .delete_object()
                .bucket(&self.inner.bucket)
                .key(&key)
                .send(),
        )
        .await
        {
            Ok(_) => None,
            Err(error) => Some(format!(
                "the proof object {key} could not be removed: {}",
                sdk_err("delete_object", error)
            )),
        };
        let answer = |text: String| match &left {
            Some(left) => format!("{text}; {left}"),
            None => text,
        };
        let mut ignored = Vec::new();
        match second {
            Err(error) if is_precondition_failed(&error) => {}
            Ok(_) => ignored.push("If-None-Match: * (a second create of the same key was stored)"),
            Err(error) => {
                return Err(answer(format!(
                    "{store} could not be checked for conditional writes: the second create of \
                     {key} failed instead of answering 412: {}",
                    sdk_err("put_object if-absent", error)
                )))
            }
        }
        match replace {
            Err(error) if is_precondition_failed(&error) => {}
            Ok(_) => {
                ignored.push("If-Match (a replacement against a version it never held was stored)")
            }
            Err(error) => {
                return Err(answer(format!(
                    "{store} could not be checked for conditional writes: replacing {key} against \
                     a version it never held failed instead of answering 412: {}",
                    sdk_err("conditional put_object", error)
                )))
            }
        }
        if !ignored.is_empty() {
            return Ok(Err(answer(format!(
                "{store} ignores {}; the queue's claims, leases and publications would succeed \
                 for two holders at once, so no write that relies on a precondition is sent to \
                 it. Use a provider or server release that honours conditional PutObject",
                ignored.join(" and ")
            ))));
        }
        match left {
            Some(left) => Err(format!("{store} honours conditional writes, but {left}")),
            None => Ok(Ok(())),
        }
    }
}
