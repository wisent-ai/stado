//! Real, prompt-free preparation on the explicitly registered Apple host. No
//! Apple authentication, notification, browser, or CuaDriver launch occurs.
//! The workload capability retired `host gui-automation …`; until this
//! revision both stories called those verbs and died before reaching a host.

mod support;

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use reqwest::StatusCode;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;
use serde_json::{json, Value};

use support::{
    plan_file, prepare, registered_host, report, retain, stado_binary, status, Report,
    APPLE_ONLY_PLAN, API_COMMAND_SECONDS,
};

mod cases;
