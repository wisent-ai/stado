//! Getting the staged source archive into storage and proving, by reading it
//! back, that what the reservation promises is what is there.

use crate::machine::requests::leases::renew_machine_request_claim;
use crate::machine::sources::archive::validate_staged_source_archive;
use crate::machine::sources::sha256_path;
use crate::machine::sources::staging::readback_machine_source;
use crate::machine::{MachineError, MachineFacade};

use super::{ClaimedReservation, SubmitRequestContext};

impl MachineFacade {
    /// Upload the staged archive if storage does not already hold it, then
    /// read the authoritative copy back and verify its size and digest.
    pub(super) async fn upload_machine_source(
        &self,
        ctx: &SubmitRequestContext,
        reserved: &mut ClaimedReservation,
    ) -> Result<(), MachineError> {
        let SubmitRequestContext {
            request_id,
            record_path,
            owner,
            ..
        } = ctx;
        let source_requested = ctx.source_requested;
        let source_sha = reserved.source_sha.as_str();
        let source_bytes = reserved.source_bytes;
        let replayed_reservation = reserved.replayed_reservation;
        let staged_source = &mut reserved.staged_source;
        if source_requested {
            let source_object = crate::remote::object_store::ObjectRef::new(
                "machine-inputs",
                &format!("{request_id}/{source_sha}.tar.gz"),
            )?;
            let source_blob = source_object.storage_path();
            let mut authoritative_readback = None;

            if replayed_reservation {
                renew_machine_request_claim(
                    &self.store,
                    record_path,
                    owner,
                    "retained-source-readback",
                )
                .await?;
                authoritative_readback = readback_machine_source(&self.store, &source_blob).await?;
                renew_machine_request_claim(
                    &self.store,
                    record_path,
                    owner,
                    "retained-source-readback-complete",
                )
                .await?;
            }

            if authoritative_readback.is_none() {
                let staged = staged_source.as_ref().ok_or_else(|| {
                    MachineError::new("INTERNAL", "staged source archive is unavailable")
                })?;
                renew_machine_request_claim(&self.store, record_path, owner, "source-upload")
                    .await?;
                self.store
                    .upload_file_if_absent(&source_blob, staged.path())
                    .await
                    .map_err(|error| {
                        MachineError::retryable("SOURCE_UPLOAD_FAILED", error.to_string())
                    })?;
                renew_machine_request_claim(
                    &self.store,
                    record_path,
                    owner,
                    "source-upload-complete",
                )
                .await?;
                renew_machine_request_claim(&self.store, record_path, owner, "source-readback")
                    .await?;
                authoritative_readback = readback_machine_source(&self.store, &source_blob).await?;
                renew_machine_request_claim(
                    &self.store,
                    record_path,
                    owner,
                    "source-readback-complete",
                )
                .await?;
            }

            let readback = authoritative_readback.ok_or_else(|| {
                MachineError::retryable(
                    "SOURCE_UPLOAD_FAILED",
                    "retained source archive is missing",
                )
            })?;
            let readback_bytes = std::fs::metadata(readback.path())
                .map_err(|error| {
                    MachineError::retryable("SOURCE_UPLOAD_FAILED", error.to_string())
                })?
                .len();
            if readback_bytes != source_bytes {
                return Err(MachineError::retryable(
                    "SOURCE_UPLOAD_FAILED",
                    "retained source archive byte count differs from its reservation",
                ));
            }
            renew_machine_request_claim(&self.store, record_path, owner, "source-validation")
                .await?;
            validate_staged_source_archive(readback.path()).map_err(|error| {
                MachineError::retryable("SOURCE_UPLOAD_FAILED", error.to_string())
            })?;
            let readback_sha = sha256_path(readback.path()).map_err(|error| {
                MachineError::retryable("SOURCE_UPLOAD_FAILED", error.to_string())
            })?;
            renew_machine_request_claim(
                &self.store,
                record_path,
                owner,
                "source-validation-complete",
            )
            .await?;
            if readback_sha != source_sha {
                return Err(MachineError::retryable(
                    "SOURCE_UPLOAD_FAILED",
                    "retained source archive digest differs from its reservation",
                ));
            }
            readback.cleanup().map_err(|error| {
                MachineError::retryable("SOURCE_UPLOAD_FAILED", error.to_string())
            })?;
            if let Some(staged) = staged_source.take() {
                staged.cleanup().map_err(|error| {
                    MachineError::retryable("SOURCE_UPLOAD_FAILED", error.to_string())
                })?;
            }
        }
        Ok(())
    }
}
