//! What this machine can be asked about itself before work is placed on it:
//! its GPUs, the Hugging Face rate budget it has left, and whether the
//! installed binary is the one the fleet expects.

pub mod gpu;
pub mod hf_rate;
pub mod version_check;
