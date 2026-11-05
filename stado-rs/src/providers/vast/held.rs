//! Whether any board of this host is held by something a renter would share
//! it with: an inference reservation, or any process holding board memory (a
//! resident engine such as vLLM or ComfyUI, or a job still on the board).
//!
//! Vast.ai lists a machine whole: every board goes into the offer. A board an
//! engine holds would be offered to a renter while it serves, so the host is
//! not listed while one is held; listing and the follow loop read this before
//! each listing and name the holder.

/// Why the host may not be listed now, or `None` when no board is held.
pub async fn refusal() -> Option<String> {
    match crate::inference::reservation::active() {
        Ok(Some(reservation)) => {
            let board = reservation
                .gpu
                .as_deref()
                .map(|board| format!("board {board}"))
                .unwrap_or_else(|| "a board".to_string());
            return Some(format!(
                "inference deployment {} holds {board}, and Vast.ai lists the machine whole, so \
                 the held board would be offered to a renter",
                reservation.deployment
            ));
        }
        Ok(None) => {}
        Err(cause) => {
            return Some(format!(
                "the inference reservation cannot be read ({cause}), so whether a board is held \
                 is unknown"
            ))
        }
    }
    let holders = crate::providers::local::accelerators::holders::measure(&[]).await;
    if let Some(cause) = &holders.error {
        return Some(format!(
            "who holds this host's boards cannot be read ({cause}), so a held board could be \
             offered"
        ));
    }
    let resident: Vec<String> = holders
        .holders
        .iter()
        .map(|holder| {
            format!(
                "{} (pid {}, {:.1} GiB on {})",
                holder.process, holder.pid, holder.used_vram_gb, holder.device
            )
        })
        .collect();
    if resident.is_empty() {
        return None;
    }
    Some(format!(
        "a process holds board memory ({}), and Vast.ai lists the machine whole, so the held \
         board would be offered to a renter",
        resident.join("; ")
    ))
}
