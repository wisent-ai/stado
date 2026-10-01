//! Where a job's data already lives: [`crosses_provider_boundary`] answers
//! whether placing a job on a provider would move its inputs or outputs
//! across a provider boundary.

use crate::capabilities::ProviderId;
use crate::models::Job;

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
