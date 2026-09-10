//! Real local-worker live-capacity journey.
//!
//! The built Stado binary submits two blocking CPU jobs to an isolated store,
//! then a real worker admits both before either can finish. The fixture leaves
//! the retired registry and environment worker caps at one: observing both
//! jobs running at once proves those values no longer control admission. The
//! same journey checks the worker's public capacity document and both terminal
//! job records rather than treating process output as the result.

mod support;

use std::fs;
use std::time::Duration;

use serde_json::json;

use support::{Journey, TARGET};

mod cases;
