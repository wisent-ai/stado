//! Real Services API convergence against the built Stado dashboard, a real
//! isolated Skarbiec vault, and this machine through Stado's same-host channel.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[path = "../support/skarbiec.rs"]
mod skarbiec_support;
use skarbiec_support::{real_skarbiec_binary, SkarbiecFixture, SkarbiecItem};

const HOST: &str = "probierz-service-convergence-host";
const PATH_ENV: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

mod converge;
mod dashboard;
mod grants;
mod readers;
mod start;

use dashboard::*;
use grants::*;
use readers::*;
