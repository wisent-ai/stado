//! One exact label, addressed without a declaration: reaping, bootout,
//! persistent autostart, retiring a product's catalog predecessors and
//! giving them back when their replacement did not start, and the calling
//! account's posture.

mod autostart;
mod bootout;
mod posture;
mod predecessors;
mod reap;
mod reinstate;

pub use autostart::*;
pub use bootout::*;
pub use posture::*;
pub use predecessors::*;
pub use reap::*;
pub use reinstate::*;

/// Whether the launchd `domain` (`system`, `gui/<uid>`) holds `label`, read
/// from the domain's listing, which names each service it holds as the last
/// field of its row. `print` of one service fails the same way for an absent
/// service as for a domain it may not read, so presence is never read from
/// it. Any account may list both domains; a listing that cannot be read is
/// refused with launchctl's own words, never told as "not held".
pub fn launchd_domain_holds(domain: &str, label: &str) -> Result<bool, String> {
    let output =
        crate::wait::output(std::process::Command::new("/bin/launchctl").args(["print", domain]))
            .map_err(|error| format!("cannot list launchd domain {domain}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "launchd domain {domain} could not be listed: launchctl ended with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.split_whitespace().last() == Some(label)))
}
