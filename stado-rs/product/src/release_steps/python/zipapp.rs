//! `stado product python zipapp --package P… --module M --name N.pyz`: a
//! Python product shipped as one executable zip application, placed at
//! `$WISENT_OUTPUT_DIR/bin/<name>`. trading-autonomy built its agent this way
//! with a script of its own, `release/build-runtime.py`.

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use stado_wait as wait;

use super::super::{output_dir, required};

/// What a packaged directory never carries: interpreter caches, and the
/// host-setup files that live beside the agent's modules but are not part of
/// the running program.
fn excluded(name: &str) -> bool {
    matches!(
        name,
        "__pycache__" | "setup_steps" | "setup.sh" | "publish-release.sh"
    ) || name.ends_with(".pyc")
        || name.ends_with(".service")
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        if excluded(&name.to_string_lossy()) {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            bail!(
                "{} is a symbolic link; a zip application carries files",
                entry.path().display()
            );
        }
        if kind.is_dir() {
            copy_tree(&entry.path(), &to.join(&name))?;
        } else {
            fs::copy(entry.path(), to.join(&name))?;
        }
    }
    Ok(())
}

pub fn build(packages: &[String], module: &str, name: &str, python: &str) -> Result<i32> {
    if packages.is_empty() {
        bail!("zipapp needs at least one --package directory");
    }
    if !name.ends_with(".pyz") || name.contains('/') {
        bail!("--name is a file name ending in .pyz, not {name:?}");
    }
    let source = Path::new(&required("WISENT_SOURCE_DIR")?).to_path_buf();
    let output = output_dir()?;
    let work = output.join(format!("zipapp-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        for package in packages {
            if package.contains('/') || package.starts_with('.') {
                bail!("--package names a top-level directory of the checkout, not {package:?}");
            }
            let target = work.join(package);
            copy_tree(&source.join(package), &target)?;
            let init = target.join("__init__.py");
            if !init.exists() {
                fs::write(&init, "")?;
            }
        }
        fs::write(
            work.join("__main__.py"),
            format!("import runpy\nrunpy.run_module({module:?}, run_name='__main__')\n"),
        )?;
        let bin = output.join("bin");
        fs::create_dir_all(&bin)?;
        let status = wait::status(
            Command::new(python)
                .args(["-m", "zipapp"])
                .arg(&work)
                .arg("--output")
                .arg(bin.join(name))
                .args(["--python", "/usr/bin/env python3", "--compress"]),
        )
        .with_context(|| format!("cannot run {python} -m zipapp"))?;
        if !status.success() {
            bail!("{python} -m zipapp failed with {status}");
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&work);
    result?;
    println!("staged {}", output.join("bin").join(name).display());
    Ok(0)
}
