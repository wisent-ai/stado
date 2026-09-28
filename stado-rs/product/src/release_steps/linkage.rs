//! `stado product linkage BUNDLE.app…`: prove an app bundle can start before
//! anyone installs it. Every `@rpath/` load command of every Mach-O file in the
//! bundle is resolved against that file's own `LC_RPATH` entries, expanding
//! `@executable_path` and `@loader_path` the way dyld does.
//!
//! Brama Desktop shipped on 2026-08-09 with `Sparkle.framework` inside the
//! bundle and a main executable whose run paths reached nowhere near it, so
//! dyld killed it before `main` on every machine while the signature, the
//! framework and the version all checked out. brama-desktop carried this check
//! as `release/bundle/verify-bundle-linkage.py`.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

const RPATH: &str = "@rpath/";

/// Whether `path` starts with a Mach-O or universal-binary magic number.
fn mach_o(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    let read = fs::File::open(path).and_then(|mut file| file.read_exact(&mut magic));
    read.is_ok()
        && matches!(
            u32::from_be_bytes(magic),
            0xfeed_face | 0xfeed_facf | 0xcefa_edfe | 0xcffa_edfe | 0xcafe_babe | 0xbeba_feca
        )
}

fn files(directory: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            files(&entry.path(), found)?;
        } else if kind.is_file() && mach_o(&entry.path()) {
            found.push(entry.path());
        }
    }
    Ok(())
}

/// The file's `LC_RPATH` paths and its `@rpath/` dependencies, from `otool -l`.
///
/// A framework's own install name (`LC_ID_DYLIB`) is printed with the same
/// `name` line as a dependency but is what the file answers to, not what it
/// loads: Sparkle.framework's `@rpath/Sparkle.framework/Versions/B/Sparkle` has
/// nothing to resolve against Sparkle's own (empty) run paths, and counting it
/// refused every Brama Desktop bundle as unresolved.
fn load_commands(binary: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let listed = Command::new("/usr/bin/otool")
        .arg("-l")
        .arg(binary)
        .output()
        .with_context(|| format!("cannot run otool -l {}", binary.display()))?;
    let text = String::from_utf8_lossy(&listed.stdout);
    let mut rpaths = Vec::new();
    let mut dylibs = Vec::new();
    let mut command = "";
    for line in text.lines().map(str::trim) {
        if let Some(current) = line.strip_prefix("cmd ") {
            command = current;
        } else if let Some(rest) = line.strip_prefix("path ").filter(|_| command == "LC_RPATH") {
            rpaths.push(rest.split(" (offset").next().unwrap_or(rest).to_owned());
        } else if let Some(rest) = line
            .strip_prefix("name ")
            .filter(|_| command != "LC_ID_DYLIB")
        {
            let name = rest.split(" (offset").next().unwrap_or(rest);
            if name.starts_with(RPATH) && !dylibs.iter().any(|known| known == name) {
                dylibs.push(name.to_owned());
            }
        }
    }
    Ok((rpaths, dylibs))
}

fn verify(bundle: &Path) -> Result<Vec<String>> {
    let executable_dir = bundle.join("Contents/MacOS");
    let mut binaries = Vec::new();
    files(bundle, &mut binaries)?;
    binaries.sort();
    let mut problems = Vec::new();
    for binary in &binaries {
        let (rpaths, dylibs) = load_commands(binary)?;
        let loader = binary.parent().unwrap_or(bundle);
        for dylib in &dylibs {
            let relative = &dylib[RPATH.len()..];
            let resolved = rpaths.iter().any(|rpath| {
                let expanded = rpath
                    .replace("@executable_path", &executable_dir.to_string_lossy())
                    .replace("@loader_path", &loader.to_string_lossy());
                Path::new(&expanded).join(relative).exists()
            });
            if !resolved {
                problems.push(format!(
                    "{} needs {dylib}, unresolved; rpaths were {:?}",
                    binary.strip_prefix(bundle).unwrap_or(binary).display(),
                    rpaths
                ));
            }
        }
    }
    Ok(problems)
}

pub fn run(bundles: &[String]) -> Result<i32> {
    if bundles.is_empty() {
        bail!("name at least one .app bundle");
    }
    let mut failed = false;
    for bundle in bundles {
        let path = Path::new(bundle);
        if !path.is_dir() {
            println!("{bundle}: not a bundle directory");
            failed = true;
            continue;
        }
        let problems = verify(path)?;
        if problems.is_empty() {
            println!("{bundle}: every @rpath dependency resolves inside the bundle");
        } else {
            failed = true;
            println!("{bundle}: BROKEN");
            for problem in problems {
                println!("  {problem}");
            }
        }
    }
    Ok(if failed { 1 } else { 0 })
}
