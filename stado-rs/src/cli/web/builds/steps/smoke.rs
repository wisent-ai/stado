//! `stado web smoke` — the post-build test every web product can declare.
//!
//! A web release that declares no test never qualifies the work it carries,
//! and most web products have no suite of their own. What every one of them
//! promises is the same: the artifact `stado web build` staged starts the way
//! `stado web deploy` will start it, and answers its pages. This step proves
//! exactly that on the builder: it extracts the staged tarball, runs its
//! launcher on a free loopback port, and requires each declared path to
//! answer with a success status and a body. A launcher that exits before
//! answering, or a page that answers an error, is refused by name with the
//! tail of the launcher's own output.
//!
//! The step waits for the site to answer or for its launcher to exit,
//! whichever comes first; it sets no clock of its own.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::cli::web::builds::contract::naming::{tarball_name, top_level};
use crate::cli::web::builds::contract::package::manifest_if_present;
use crate::cli::web::builds::contract::release::product;
use crate::cli::web::builds::contract::worker::worker;
use crate::cli::web::LAUNCHER;
use crate::cli::CmdError;

/// Tuning constant: how much of the launcher's output a refusal quotes.
const LOG_TAIL_BYTES: usize = 4000;

/// A launched site, ended with its whole process group on every path out.
struct Site {
    child: Child,
    log: PathBuf,
}

impl Drop for Site {
    fn drop(&mut self) {
        // The launcher execs `npm`, which starts the server as a child: the
        // group, not the one process, is what has to end.
        let group = nix::unistd::Pid::from_raw(self.child.id() as i32);
        let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGTERM);
        let _ = self.child.wait();
    }
}

impl Site {
    fn tail(&self) -> String {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let start = text.len().saturating_sub(LOG_TAIL_BYTES);
        let start = (start..text.len())
            .find(|&index| text.is_char_boundary(index))
            .unwrap_or(0);
        text[start..].trim().to_string()
    }
}

fn free_port() -> Result<u16, CmdError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| CmdError::click(format!("no free loopback port: {error}")))?;
    Ok(listener.local_addr()?.port())
}

fn extract(tarball: &Path, into: &Path) -> Result<(), CmdError> {
    if into.exists() {
        std::fs::remove_dir_all(into)?;
    }
    std::fs::create_dir_all(into)?;
    let file = std::fs::File::open(tarball).map_err(|error| {
        CmdError::click(format!(
            "the staged artifact {} cannot be opened: {error}; `stado web build` has to run before this step",
            tarball.display()
        ))
    })?;
    tar::Archive::new(flate2::read::GzDecoder::new(file))
        .unpack(into)
        .map_err(|error| {
            CmdError::click(format!(
                "{} cannot be extracted: {error}",
                tarball.display()
            ))
        })
}

fn launch(root: &Path, port: u16, log: PathBuf) -> Result<Site, CmdError> {
    use std::os::unix::process::CommandExt;
    let launcher = root.join(LAUNCHER);
    let output = std::fs::File::create(&log)?;
    let child = Command::new(&launcher)
        .env("PORT", port.to_string())
        .env_remove("WEB_ENV_FILE")
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output)
        .process_group(0)
        .spawn()
        .map_err(|error| {
            CmdError::click(format!("{} cannot be started: {error}", launcher.display()))
        })?;
    Ok(Site { child, log })
}

/// One GET of `path`: the status and the body's length, or why there is no
/// answer.
async fn fetch(client: &reqwest::Client, port: u16, path: &str) -> Result<(u16, usize), String> {
    let response = client
        .get(format!("http://127.0.0.1:{port}{path}"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let body = response.bytes().await.map_err(|error| error.to_string())?;
    Ok((status, body.len()))
}

/// Until the launcher's server accepts a connection on `port`, or the
/// launcher exits; `Some` carries the exit.
async fn until_listening(
    site: &mut Site,
    port: u16,
) -> Result<Option<std::process::ExitStatus>, CmdError> {
    loop {
        if let Some(status) = site.child.try_wait()? {
            return Ok(Some(status));
        }
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return Ok(None);
        }
        tokio::task::yield_now().await;
    }
}

pub(crate) async fn smoke(paths: &[String]) -> Result<(), CmdError> {
    let worker = worker()?;
    worker.require_web_platform()?;
    let manifest = manifest_if_present(&worker.source)?;
    let product = product(&worker.source, manifest.as_ref())?;
    let tarball = worker.output.join("dist").join(tarball_name(&product));
    let scratch = worker.output.join("smoke");
    extract(&tarball, &scratch)?;
    let root = scratch.join(top_level(&product, &worker.version));
    let port = free_port()?;
    let mut site = launch(&root, port, worker.output.join("smoke.log"))?;
    if let Some(status) = until_listening(&mut site, port).await? {
        return Err(CmdError::click(format!(
            "stado web smoke: {product}'s launcher exited with {status} before listening on port {port}:\n{}",
            site.tail()
        )));
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| CmdError::click(format!("no HTTP client: {error}")))?;
    let paths: Vec<String> = if paths.is_empty() {
        vec!["/".to_string()]
    } else {
        paths.to_vec()
    };
    let mut failures = Vec::new();
    for path in &paths {
        match fetch(&client, port, path).await {
            Ok((status, bytes)) if (200..300).contains(&status) && bytes > 0 => {
                println!("stado web smoke: {product} {path} answered {status} ({bytes} bytes)");
            }
            Ok((status, bytes)) if (300..400).contains(&status) => {
                println!(
                    "stado web smoke: {product} {path} redirected with {status} ({bytes} bytes)"
                );
            }
            Ok((status, bytes)) => {
                failures.push(format!("{path} answered {status} with {bytes} bytes"))
            }
            Err(error) => failures.push(format!("{path} did not answer: {error}")),
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "stado web smoke: {product} started but {}:\n{}",
        failures.join("; "),
        site.tail()
    )))
}
