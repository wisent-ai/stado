//! What a job asks this host for, floored at one unit each.

use crate::models::Job;

pub fn requested_cpu_cores(job: &Job) -> i64 {
    job.cpu_cores.max(1)
}

pub fn requested_memory_gb(job: &Job) -> f64 {
    job.memory_gb.max(1) as f64
}
