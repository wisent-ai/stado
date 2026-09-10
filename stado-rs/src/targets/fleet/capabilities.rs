use crate::targets::*;

// ---------------------------------------------------------------------------
// capabilities.py — provider-neutral workload admission
// ---------------------------------------------------------------------------

/// What a dispatch target declares it can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetCapabilities {
    pub target_id: String,
    pub operating_system: String,
    pub architecture: String,
    pub cpu_cores: i64,
    pub memory_gb: i64,
    pub disk_gb: i64,
    pub accelerator: String,
    pub execution_modes: BTreeSet<String>,
    pub supports_preemptible: bool,
    pub region_selectable: bool,
    pub supports_system_packages: bool,
}

impl Default for TargetCapabilities {
    fn default() -> Self {
        Self {
            target_id: String::new(),
            operating_system: String::new(),
            architecture: String::new(),
            cpu_cores: 0,
            memory_gb: 0,
            disk_gb: 0,
            accelerator: String::new(),
            execution_modes: BTreeSet::from(["stado-agent".to_string()]),
            supports_preemptible: false,
            region_selectable: false,
            supports_system_packages: false,
        }
    }
}

/// Raised by [`AdmissionDecision::require`] (Python `ValueError`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AdmissionRejection(pub String);

/// Outcome of [`admit_job`]: every incompatibility, not just the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionDecision {
    pub accepted: bool,
    pub reasons: Vec<String>,
}

impl AdmissionDecision {
    pub fn require(&self) -> Result<(), AdmissionRejection> {
        if self.accepted {
            Ok(())
        } else {
            Err(AdmissionRejection(self.reasons.join("; ")))
        }
    }
}

/// Return every incompatibility instead of failing at the first field.
pub fn admit_job(job: &Job, target: &TargetCapabilities) -> AdmissionDecision {
    let mut reasons: Vec<String> = Vec::new();
    let required_os = job.platform_os.to_lowercase();
    let required_arch = job.architecture.to_lowercase();
    let required_cpu = job.cpu_cores;
    let required_memory = job.memory_gb;
    let required_disk = job.disk_gb;
    let executor = if job.executor.is_empty() {
        "stado-agent"
    } else {
        job.executor.as_str()
    };
    let gpu_mem = job.gpu_mem_gb;
    let gpu_type = job.gpu_type.as_str();

    if !required_os.is_empty() && required_os != target.operating_system {
        reasons.push(format!(
            "requires os={required_os}, target is {}",
            target.operating_system
        ));
    }
    if !required_arch.is_empty() && required_arch != target.architecture {
        reasons.push(format!(
            "requires architecture={required_arch}, target is {}",
            target.architecture
        ));
    }
    if required_cpu > target.cpu_cores {
        reasons.push(format!(
            "requires {required_cpu} CPU cores, target has {}",
            target.cpu_cores
        ));
    }
    if required_memory > target.memory_gb {
        reasons.push(format!(
            "requires {required_memory} GB memory, target has {}",
            target.memory_gb
        ));
    }
    if required_disk > target.disk_gb {
        reasons.push(format!(
            "requires {required_disk} GB disk, target has {}",
            target.disk_gb
        ));
    }
    // Faithful to the Python: target.accelerator is NOT consulted here —
    // any GPU requirement rejects against a capability set (the declared
    // accelerator string is informational only).
    if gpu_mem > 0 || !gpu_type.is_empty() {
        reasons.push("target has no accelerator".to_string());
    }
    if !target.execution_modes.contains(executor) {
        reasons.push(format!("executor '{executor}' is unsupported"));
    }
    if job.preemptible && !target.supports_preemptible {
        reasons.push("target does not support preemptible lifecycle".to_string());
    }
    if !job.region.is_empty() && !target.region_selectable {
        reasons.push("target region is not selectable".to_string());
    }
    if !job.apt_packages.is_empty() && !target.supports_system_packages {
        reasons.push("target does not support provider-managed system packages".to_string());
    }
    AdmissionDecision {
        accepted: reasons.is_empty(),
        reasons,
    }
}

static BOX_CAPABILITIES: LazyLock<TargetCapabilities> = LazyLock::new(|| TargetCapabilities {
    target_id: "box-linux-sandbox".to_string(),
    operating_system: "linux".to_string(),
    architecture: "x86_64".to_string(),
    cpu_cores: 4,
    memory_gb: 8,
    disk_gb: 80,
    accelerator: String::new(),
    execution_modes: BTreeSet::from([
        "stado-agent".to_string(),
        "box-command".to_string(),
        "box-prompt".to_string(),
    ]),
    supports_preemptible: false,
    region_selectable: false,
    supports_system_packages: false,
});

/// Capability set of the Linux sandbox box (Python `BOX_CAPABILITIES`).
pub fn box_capabilities() -> &'static TargetCapabilities {
    &BOX_CAPABILITIES
}
