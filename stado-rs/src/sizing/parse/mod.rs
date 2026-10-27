//! Text extraction: the `--model` flag out of a command line, the provable
//! VRAM figures out of a PyTorch out-of-memory message, and whether a job
//! failed for want of memory at all.
//!
//! PyTorch states an out-of-memory failure in one documented sentence
//! (`Tried to allocate <n> <unit> ... this process has <n> GiB memory in
//! use`), so its figures are read from that structure. Whether any other
//! error is an out-of-memory failure is a choice a model makes through Oko's
//! schema-bound call (`model_judgement::choose`, `oom-prompt.md` beside this
//! file): a regex of CUDA and framework phrases decided this before, and
//! missed every runtime that words its failure differently.

use std::sync::LazyLock;

use regex::Regex;

use crate::primitives::model_judgement;
use crate::providers::local::host_memory::constants::MIB;

/// `--model <value>` or `--model=<value>`: the one reading of a command's
/// model every part of Stado shares ([`model_of`]).
static MODEL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"--model(?:\s+|=)(?P<model>\S+)").expect("static regex compiles")
});
/// A decimal figure as PyTorch writes one: digits, and at most one fraction.
/// Every text this admits is an `f64` literal, so reading it cannot fail.
static OOM_PROC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)this process has (?P<held>[0-9]+(?:\.[0-9]+)?) GiB memory in use")
        .expect("static regex compiles")
});
static OOM_ALLOC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)Tried to allocate (?P<amount>[0-9]+(?:\.[0-9]+)?) (?P<unit>[MG])iB")
        .expect("static regex compiles")
});

const PROMPT: &str = include_str!("oom-prompt.md");
const ERROR_SLOT: &str = "{error}";
/// The schema the model answers under and the two options it chooses from.
const JUDGEMENT: &str = "job_failure_cause";
const OUT_OF_MEMORY: &str = "OUT_OF_MEMORY";
const OTHER: &str = "OTHER";
/// The unit letter PyTorch writes for a gibibyte figure.
const GIB_UNIT: &str = "G";

/// The model a command line names with `--model <value>` or
/// `--model=<value>`, quote-stripped; `None` when it names none, or names an
/// empty one.
pub fn model_of(command: &str) -> Option<String> {
    let model = MODEL_RE.captures(command)?["model"]
        .trim_matches(['\'', '"'])
        .to_string();
    (!model.is_empty()).then_some(model)
}

/// MiB in one GiB: a GiB holds as many MiB as a MiB holds KiB, the square
/// root of the bytes in the declared mebibyte.
fn mib_per_gib() -> f64 {
    (MIB as f64).sqrt()
}

/// A figure the OOM patterns captured, as a number.
fn figure(text: &str) -> f64 {
    text.parse()
        .expect("the OOM patterns capture only a decimal figure")
}

/// Python `_oom_required_gb`: VRAM the OOMing process provably needed,
/// from the PyTorch OOM message, rounded up to whole GiB; none when the
/// message carries no "this process has ..." figure.
pub fn oom_required_gb(text: &str) -> Option<i64> {
    let held = figure(&OOM_PROC_RE.captures(text)?["held"]);
    let allocation = OOM_ALLOC_RE.captures(text).map(|caps| {
        let amount = figure(&caps["amount"]);
        if &caps["unit"] == GIB_UNIT {
            amount
        } else {
            amount / mib_per_gib()
        }
    });
    let need = match allocation {
        Some(allocation) => held + allocation,
        None => held,
    };
    Some(need.ceil() as i64)
}

/// Whether the job failed for want of memory: a PyTorch out-of-memory
/// message by its own figures, any other error as the model reads it. The
/// cause, when nobody could read the error.
pub fn ran_out_of_memory(text: &str) -> Result<bool, String> {
    if OOM_PROC_RE.is_match(text) {
        return Ok(true);
    }
    let judged = model_judgement::choose(
        &PROMPT.replace(ERROR_SLOT, text),
        JUDGEMENT,
        &[OUT_OF_MEMORY, OTHER],
    )?;
    Ok(judged.choice == OUT_OF_MEMORY)
}
