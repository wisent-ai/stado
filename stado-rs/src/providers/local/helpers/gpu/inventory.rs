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

/// Pure: first line of a `nvidia-smi --format=csv,noheader,nounits` reply
/// as an integer MiB count. None when unparsable (Python ValueError path).
pub fn parse_smi_mib_first(stdout: &str) -> Option<i64> {
    stdout.trim().lines().next()?.trim().parse().ok()
}

/// Python `_detect_gpu_type`: nvidia-smi on Linux, sysctl brand string on
/// macOS, else "cpu".
pub async fn detect_gpu_type() -> String {
    if let Ok(out) = tokio::process::Command::new("nvidia-smi")
        .args(["--query-gpu=name", "--format=csv,noheader"])
        .output()
        .await
    {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            // Python: r.stdout.strip().split("\n")[0] — an empty result is
            // returned verbatim (""), not treated as "no GPU".
            return normalize_gpu_name(stdout.trim().lines().next().unwrap_or(""));
        }
    }
    if let Ok(out) = tokio::process::Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .await
    {
        if String::from_utf8_lossy(&out.stdout).contains("Apple") {
            return "apple-mps".to_string();
        }
    }
    "cpu".to_string()
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
/// --format=csv,noheader,nounits`: one [`GpuCard`] per readable row.
pub fn parse_gpu_cards(stdout: &str) -> Vec<GpuCard> {
    let mut cards = Vec::new();
    for line in stdout.lines() {
        let mut fields = line.split(',').map(str::trim);
        let (Some(uuid), Some(total), Some(free)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(total_mib), Ok(free_mib)) = (total.parse::<i64>(), free.parse::<i64>()) else {
            continue;
        };
        if uuid.is_empty() {
            continue;
        }
        cards.push(GpuCard {
            uuid: uuid.to_string(),
            total_vram_gb: total_mib / 1024,
            free_vram_gb: free_mib / 1024,
        });
    }
    cards
}

/// Every accelerator this host has, newest driver reading. Empty when
/// `nvidia-smi` is absent or answers nothing parsable.
///
/// One call, every card: the previous readings took the FIRST line of a
/// per-GPU query, so on the fleet's two-card host the agent measured card 0
/// and nothing else -- it advertised 35 GiB free while a second, idle 95 GiB
/// board sat beside it, and two concurrent slots were both admitted against
/// card 0's numbers.
pub async fn smi_gpu_cards() -> Vec<GpuCard> {
    let Ok(out) = tokio::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=uuid,memory.total,memory.free",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .await
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_gpu_cards(&String::from_utf8_lossy(&out.stdout))
}

/// Python `_detect_local_vram_gb`: total VRAM in GB of the largest card this
/// host has, 0 if none.
///
/// The largest single card, not the sum: a job that does not shard can only
/// use one board, and every admission comparison downstream treats this as
/// "the biggest thing that fits".
pub async fn detect_local_vram_gb() -> i64 {
    smi_gpu_cards()
        .await
        .iter()
        .map(|card| card.total_vram_gb)
        .max()
        .unwrap_or(0)
}

/// Python `_smi_free_vram_gb`: live free VRAM in GB on the emptiest card,
/// -1 if the driver is unreadable.
///
/// The emptiest card is the honest answer to "will this job fit": a workload
/// holding card 0 -- ours, an external one like ComfyUI, or a Vast renter -- does
/// not shrink what card 1 can take.
pub async fn smi_free_vram_gb() -> i64 {
    smi_gpu_cards()
        .await
        .iter()
        .map(|card| card.free_vram_gb)
        .max()
        .unwrap_or(-1)
}
