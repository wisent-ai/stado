use reqwest::Client;
use serde::Serialize;
use std::process::Command;
use std::time::Duration;

#[derive(Serialize)]
struct Heartbeat {
    machine_id: String,
    agent_version: String,
    gpu_utilization: f32,
    gpu_temp: f32,
    gpu_memory_used_mb: i64,
    gpu_memory_total_mb: i64,
    cpu_utilization: f32,
    ram_used_mb: i64,
    ram_total_mb: i64,
}

pub async fn run(client: &Client, server: &str, machine_id: &str, token: &str) {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    loop {
        interval.tick().await;
        let hb = collect(machine_id);
        let url = format!("{server}/api/v1/agent/heartbeat");
        match client.post(&url)
            .bearer_auth(token)
            .json(&hb)
            .send().await
        {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => tracing::warn!("heartbeat {}", r.status()),
            Err(e) => tracing::warn!("heartbeat failed: {e}"),
        }
    }
}

fn collect(machine_id: &str) -> Heartbeat {
    let (gpu_util, gpu_temp, gpu_mem_used, gpu_mem_total) = gpu_info();
    let (ram_used, ram_total) = ram_info();

    Heartbeat {
        machine_id: machine_id.to_string(),
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
        gpu_utilization: gpu_util,
        gpu_temp,
        gpu_memory_used_mb: gpu_mem_used,
        gpu_memory_total_mb: gpu_mem_total,
        cpu_utilization: 0.0,
        ram_used_mb: ram_used,
        ram_total_mb: ram_total,
    }
}

fn gpu_info() -> (f32, f32, i64, i64) {
    let out = Command::new("nvidia-smi")
        .args(["--query-gpu=utilization.gpu,temperature.gpu,memory.used,memory.total",
               "--format=csv,noheader,nounits"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            let parts: Vec<&str> = s.trim().split(", ").collect();
            if parts.len() >= 4 {
                let util = parts[0].parse().unwrap_or(0.0);
                let temp = parts[1].parse().unwrap_or(0.0);
                let used = parts[2].parse().unwrap_or(0);
                let total = parts[3].parse().unwrap_or(0);
                return (util, temp, used, total);
            }
            (0.0, 0.0, 0, 0)
        }
        _ => (0.0, 0.0, 0, 0),
    }
}

fn ram_info() -> (i64, i64) {
    let out = Command::new("free").arg("-m").output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            for line in s.lines() {
                if line.starts_with("Mem:") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 3 {
                        let total = parts[1].parse().unwrap_or(0);
                        let used = parts[2].parse().unwrap_or(0);
                        return (used, total);
                    }
                }
            }
            (0, 0)
        }
        _ => (0, 0),
    }
}
