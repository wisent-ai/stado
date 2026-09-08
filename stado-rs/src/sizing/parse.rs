//! Text extraction: the `--model` flag out of a command line and the
//! provable VRAM figures out of a PyTorch OOM message.

use std::sync::LazyLock;

use regex::Regex;

static MODEL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"--model\s+(\S+)").expect("static regex compiles"));
static OOM_PROC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)this process has ([0-9.]+) GiB memory in use").expect("static regex compiles")
});
static OOM_ALLOC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)Tried to allocate ([0-9.]+) (MiB|GiB)").expect("static regex compiles")
});
static OOM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)out of memory|OutOfMemoryError|CUDA error: out of memory|\
         CUDA_ERROR_OUT_OF_MEMORY|cuBLAS.*alloc|cudaErrorMemoryAllocation",
    )
    .expect("static regex compiles")
});

/// Python `_model_of`: `--model <value>` out of a command line,
/// quote-stripped. "" when absent.
pub fn model_of(command: &str) -> String {
    let Some(caps) = MODEL_RE.captures(command) else {
        return String::new();
    };
    caps[1].trim_matches(['\'', '"']).to_string()
}

/// Python `_oom_required_gb`: VRAM the OOMing process provably needed,
/// from the PyTorch OOM message. 0 when the message carries no
/// "this process has ..." figure.
pub fn oom_required_gb(text: &str) -> i64 {
    let proc = OOM_PROC_RE.captures(text);
    let alloc = OOM_ALLOC_RE.captures(text);
    let Some(proc) = proc else { return 0 };
    let mut need: f64 = proc[1].parse().unwrap_or(0.0);
    if let Some(alloc) = alloc {
        let x: f64 = alloc[1].parse().unwrap_or(0.0);
        need += if alloc[2].eq_ignore_ascii_case("gib") {
            x
        } else {
            x / 1024.0
        };
    }
    (need.ceil() as i64).max(1)
}

/// Python `_OOM_RE.search`.
pub fn is_oom_error(text: &str) -> bool {
    OOM_RE.is_match(text)
}
