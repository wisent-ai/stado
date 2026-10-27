//! The accelerators this host actually has, as the driver reports them.
//!
//! Every reading here comes from `nvidia-smi` (or the macOS brand string) and
//! nothing else: the name the fleet routes on, the per-card totals admission
//! compares against, and the live free VRAM a claim decision needs.

/// Pure: Python `name.lower().replace(" ", "-").replace("geforce-", "nvidia-")`.
pub fn normalize_gpu_name(name: &str) -> String {
    name.to_lowercase()
        .replace(' ', "-")
        .replace("geforce-", "nvidia-")
}

/// Python `_detect_gpu_type`: nvidia-smi on Linux, sysctl brand string on
/// macOS, else "cpu". A host without `nvidia-smi` has no NVIDIA driver; one
/// whose `nvidia-smi` is there and fails is refused, not routed as a CPU host.
pub async fn detect_gpu_type() -> Result<String, String> {
    match crate::wait::output_async(
        tokio::process::Command::new("nvidia-smi")
            .args(["--query-gpu=name", "--format=csv,noheader"]),
    )
    .await
    {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            return match stdout.trim().lines().next() {
                Some(name) if !name.trim().is_empty() => Ok(normalize_gpu_name(name)),
                _ => Err("nvidia-smi ran and named no GPU".to_string()),
            };
        }
        Ok(out) => {
            return Err(format!(
                "nvidia-smi is installed and exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("nvidia-smi could not run: {error}")),
    }
    if let Ok(out) = crate::wait::output_async(
        tokio::process::Command::new("sysctl").args(["-n", "machdep.cpu.brand_string"]),
    )
    .await
    {
        if String::from_utf8_lossy(&out.stdout).contains("Apple") {
            return Ok("apple-mps".to_string());
        }
    }
    Ok("cpu".to_string())
}

/// One accelerator, as the driver reports it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GpuCard {
    /// Driver UUID (`GPU-...`), which is what `CUDA_VISIBLE_DEVICES` should
    /// carry: an index is positional and reorders with the enumeration mode,
    /// while a UUID names one board.
    pub uuid: String,
    pub total_vram_gb: i64,
    pub free_vram_gb: i64,
}

/// Pure parser for `nvidia-smi --query-gpu=uuid,memory.total,memory.free
/// --format=csv,noheader,nounits`: one [`GpuCard`] per row, its MiB in whole
/// GiB. A row the driver printed and this cannot read is refused naming it:
/// dropping it would hide a card the host has from every admission decision.
pub fn parse_gpu_cards(stdout: &str) -> Result<Vec<GpuCard>, String> {
    use crate::providers::local::host_memory::constants::{GIB, MIB};
    let mib_per_gib = GIB / MIB;
    let mut cards = Vec::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        let [uuid, total, free] = fields.as_slice() else {
            return Err(format!("nvidia-smi printed {line:?}, not uuid, memory.total, memory.free"));
        };
        if uuid.is_empty() {
            return Err(format!("nvidia-smi printed {line:?} with no card uuid"));
        }
        let mib = |name: &str, value: &str| {
            value
                .parse::<i64>()
                .map_err(|error| format!("nvidia-smi's {name} {value:?} for {uuid} is not a MiB count: {error}"))
        };
        cards.push(GpuCard {
            uuid: uuid.to_string(),
            total_vram_gb: mib("memory.total", total)? / mib_per_gib,
            free_vram_gb: mib("memory.free", free)? / mib_per_gib,
        });
    }
    Ok(cards)
}

/// Every accelerator this host has, newest driver reading. Empty when the
/// host has no `nvidia-smi`; a driver that is installed and fails, or does
/// not run, is refused with its cause rather than read as no cards.
///
/// One call, every card: the previous readings took the FIRST line of a
/// per-GPU query, so on the fleet's two-card host the agent measured card 0
/// and nothing else -- it advertised 35 GiB free while a second, idle 95 GiB
/// board sat beside it, and two concurrent slots were both admitted against
/// card 0's numbers.
pub async fn smi_gpu_cards() -> Result<Vec<GpuCard>, String> {
    let out = match crate::wait::output_async(tokio::process::Command::new("nvidia-smi").args([
        "--query-gpu=uuid,memory.total,memory.free",
        "--format=csv,noheader,nounits",
    ]))
    .await
    {
        Ok(out) => out,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("nvidia-smi could not run: {error}")),
    };
    if !out.status.success() {
        return Err(format!(
            "nvidia-smi exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_gpu_cards(&String::from_utf8_lossy(&out.stdout))
}

/// Python `_detect_local_vram_gb`: total VRAM in GB of the largest card this
/// host has; a host with no card has the accelerator memory of no card.
///
/// The largest single card, not the sum: a job that does not shard can only
/// use one board, and every admission comparison downstream treats this as
/// "the biggest thing that fits".
pub async fn detect_local_vram_gb() -> Result<i64, String> {
    let cards = smi_gpu_cards().await?;
    let totals = cards.iter().map(|card| card.total_vram_gb);
    Ok(match totals.clone().max() {
        Some(largest) => largest,
        // No card: the sum over the cards it has, which is none.
        None => totals.sum(),
    })
}
