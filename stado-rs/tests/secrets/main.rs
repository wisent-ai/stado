//! Credential commands against the real Skarbiec binary and an isolated vault.
//! SKARBIEC_BIN selects a qualified artifact; the installed binary is the default.
//! GnuPG's short product-owned root is removed on drop.

mod inventory;
mod support;

use std::fs;

use serde_json::Value;

use support::{assert_success, SkarbiecFixture};

mod cases;
