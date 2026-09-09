//! Activation-extraction entrypoint rules: the deprecated foreground uploader
//! and the GPU-sharing exception for the raw extractor.

pub const DEPRECATED_ACTIVATION_ENTRYPOINT: &str = "wisent.scripts.activations.extract_and_upload";

pub fn deprecated_activation_command_reason(command: &str) -> &'static str {
    if !command.contains(DEPRECATED_ACTIVATION_ENTRYPOINT) {
        return "";
    }
    "refusing deprecated foreground activation uploader; use \
     wisent.scripts.activations.raw.extract_and_upload so extraction \
     hands upload to the detached worker pool"
}

/// Activation extraction jobs are VRAM-sized, not whole-GPU-exclusive.
pub fn activation_extraction_must_share_gpu(command: &str) -> bool {
    command.contains("wisent.scripts.activations.raw.extract_and_upload")
}
