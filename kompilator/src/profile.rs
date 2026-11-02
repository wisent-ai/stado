//! The profile a build compiles with: Cargo's `dev` and `release` defaults,
//! changed by the root manifest's `[profile.dev]` or `[profile.release]`
//! and its `build-override`, which is what build scripts, procedural macros
//! and what they use are compiled with.

use std::path::Path;

use crate::toml::Value;

#[derive(Clone, Debug, Hash)]
pub struct Settings {
    pub opt_level: String,
    /// `0`, `1`, `2`, `line-tables-only` or `line-directives-only`.
    pub debug: String,
    pub debug_assertions: bool,
    pub overflow_checks: bool,
    /// `off`, `thin` or `fat`.
    pub lto: String,
    pub codegen_units: Option<u32>,
    pub panic: String,
    /// `none`, `debuginfo` or `symbols`.
    pub strip: String,
    pub incremental: bool,
}

#[derive(Clone, Debug, Hash)]
pub struct Profile {
    pub name: String,
    pub target: Settings,
    pub host: Settings,
}

fn word(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Integer(number) => Some(number.to_string()),
        Value::Boolean(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn apply(settings: &mut Settings, table: Option<&Value>) -> Result<(), String> {
    let Some(table) = table.and_then(Value::as_table) else { return Ok(()) };
    for (key, value) in table {
        let given = word(value);
        let flag = value.as_bool();
        match key.as_str() {
            "opt-level" => settings.opt_level = given.ok_or("opt-level takes a number or s or z")?,
            "debug" => {
                settings.debug = match given.as_deref() {
                    Some("true") => String::from("2"),
                    Some("false") | Some("none") => String::from("0"),
                    Some("limited") => String::from("1"),
                    Some("full") => String::from("2"),
                    Some(other) => other.to_string(),
                    None => return Err(String::from("debug takes a level")),
                }
            }
            "debug-assertions" => settings.debug_assertions = flag.ok_or("debug-assertions takes true or false")?,
            "overflow-checks" => settings.overflow_checks = flag.ok_or("overflow-checks takes true or false")?,
            "lto" => {
                settings.lto = match given.as_deref() {
                    Some("true") | Some("fat") => String::from("fat"),
                    Some("false") | Some("off") => String::from("off"),
                    Some("thin") => String::from("thin"),
                    other => return Err(format!("lto takes true, false, thin, fat or off, got {other:?}")),
                }
            }
            "codegen-units" => {
                settings.codegen_units =
                    Some(given.and_then(|units| units.parse().ok()).ok_or("codegen-units takes a positive number")?)
            }
            "panic" => settings.panic = given.ok_or("panic takes unwind or abort")?,
            "strip" => {
                settings.strip = match given.as_deref() {
                    Some("true") => String::from("symbols"),
                    Some("false") => String::from("none"),
                    Some(other) => other.to_string(),
                    None => return Err(String::from("strip takes none, debuginfo or symbols")),
                }
            }
            "incremental" => settings.incremental = flag.ok_or("incremental takes true or false")?,
            // Read where they apply; not a setting of one unit.
            "build-override" | "package" | "inherits" | "rpath" | "split-debuginfo" | "trim-paths" => {}
            other => return Err(format!("the profile setting {other} is not one this build reads")),
        }
    }
    Ok(())
}

impl Profile {
    pub fn read(root_dir: &Path, release: bool) -> Result<Profile, String> {
        let name = if release { "release" } else { "dev" };
        let mut target = if release {
            Settings {
                opt_level: String::from("3"),
                debug: String::from("0"),
                debug_assertions: false,
                overflow_checks: false,
                lto: String::from("off"),
                codegen_units: Some(16),
                panic: String::from("unwind"),
                strip: String::from("none"),
                incremental: false,
            }
        } else {
            Settings {
                opt_level: String::from("0"),
                debug: String::from("2"),
                debug_assertions: true,
                overflow_checks: true,
                lto: String::from("off"),
                codegen_units: Some(256),
                panic: String::from("unwind"),
                strip: String::from("none"),
                incremental: true,
            }
        };
        let manifest = root_dir.join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest).map_err(|error| format!("{} cannot be read: {error}", manifest.display()))?;
        let document = crate::toml::parse(&text).map_err(|why| format!("{}: {why}", manifest.display()))?;
        let table = document.get("profile").and_then(|profiles| profiles.get(name));
        apply(&mut target, table).map_err(|why| format!("[profile.{name}] in {}: {why}", manifest.display()))?;
        // A release without debug info asked strips it from the binary.
        if release && target.debug == "0" && table.and_then(|table| table.get("strip")).is_none() {
            target.strip = String::from("debuginfo");
        }
        // Build scripts and procedural macros: unoptimised, without debug
        // info, unless the build override says otherwise.
        let mut host = Settings {
            opt_level: String::from("0"),
            debug: String::from("0"),
            codegen_units: Some(256),
            lto: String::from("off"),
            strip: String::from("none"),
            incremental: false,
            ..target.clone()
        };
        if !release {
            host.debug = target.debug.clone();
        }
        apply(&mut host, table.and_then(|table| table.get("build-override")))
            .map_err(|why| format!("[profile.{name}.build-override] in {}: {why}", manifest.display()))?;
        Ok(Profile { name: name.to_string(), target, host })
    }
}
