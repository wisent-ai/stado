//! What a provider shape holds and where a job's data already lives.
//!
//! [`machine_capacity`] reads the CPU and memory a machine type names, for
//! the jobs that constrain them explicitly, and
//! [`crosses_provider_boundary`] answers whether placing a job on a provider
//! would move its inputs or outputs across a provider boundary.

use crate::capabilities::ProviderId;
use crate::models::Job;

pub(super) fn machine_capacity(provider: ProviderId, machine_type: &str) -> Option<(i64, i64)> {
    if provider != ProviderId::Gcp {
        return None;
    }
    let cpu = machine_type.rsplit('-').next()?.parse::<i64>().ok()?;
    let two = (u16::BITS / u8::BITS) as i64;
    let memory_per_cpu = if machine_type.contains("highmem") {
        u8::BITS as i64
    } else if machine_type.contains("highcpu") {
        true as i64
    } else if machine_type.contains("standard") {
        two * two
    } else {
        return None;
    };
    Some((cpu, cpu * memory_per_cpu))
}

pub(super) fn crosses_provider_boundary(job: &Job, provider: ProviderId) -> bool {
    let uris = [&job.startup_script_uri, &job.output_uri];
    uris.iter().any(|uri| {
        (!uri.is_empty())
            && ((uri.starts_with("gs://") && provider != ProviderId::Gcp)
                || (uri.starts_with("s3://") && provider != ProviderId::Aws)
                || (uri.starts_with("https://")
                    && uri.contains("blob.core.windows.net")
                    && provider != ProviderId::Azure))
    })
}
