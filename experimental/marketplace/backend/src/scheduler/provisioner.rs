use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

use crate::gcp;

const STARTUP_TEMPLATE: &str = r#"#!/bin/bash
set -euxo pipefail
exec > /var/log/wisent-agent-setup.log 2>&1

export DEBIAN_FRONTEND=noninteractive

# Install Docker if not present. The pytorch deep-learning images we use
# ship with CUDA + drivers but no Docker; install via the official APT repo.
if ! command -v docker >/dev/null 2>&1; then
    apt-get update -qq
    apt-get install -y -qq ca-certificates curl gnupg
    install -m 0755 -d /etc/apt/keyrings
    curl -fsSL https://download.docker.com/linux/ubuntu/gpg | gpg --dearmor -o /etc/apt/keyrings/docker.gpg
    chmod a+r /etc/apt/keyrings/docker.gpg
    echo "deb [arch=amd64 signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu jammy stable" > /etc/apt/sources.list.d/docker.list
    apt-get update -qq
    apt-get install -y -qq docker-ce docker-ce-cli containerd.io
    systemctl enable --now docker
fi

# Install nvidia-container-toolkit if missing, then wire it into Docker
if ! dpkg -l nvidia-container-toolkit >/dev/null 2>&1; then
    apt-get install -y -qq nvidia-container-toolkit || true
fi
nvidia-ctk runtime configure --runtime=docker || true
systemctl restart docker

# Wire docker to authenticate with GCP Artifact Registry using the VM's
# attached service account token from the metadata server. Without this,
# docker pull against a private AR repo returns 404 (anonymous miss).
install -d /root/.docker
cat > /root/.docker/config.json << 'DOCKERCFG'
{ "credHelpers": {
    "us-central1-docker.pkg.dev": "gcr",
    "us-docker.pkg.dev": "gcr",
    "gcr.io": "gcr",
    "us.gcr.io": "gcr"
} }
DOCKERCFG
curl -sL https://github.com/GoogleCloudPlatform/docker-credential-gcr/releases/download/v2.1.22/docker-credential-gcr_linux_amd64-2.1.22.tar.gz \
  | tar -xz -C /usr/local/bin docker-credential-gcr
chmod +x /usr/local/bin/docker-credential-gcr

# Download and run agent
curl -sL "AGENT_BINARY_URL" -o /usr/local/bin/wisent-agent
chmod +x /usr/local/bin/wisent-agent

cat > /etc/systemd/system/wisent-agent.service << EOF
[Unit]
Description=Wisent Compute Agent
After=docker.service
[Service]
Environment=BACKEND_URL=BACKEND_URL_PLACEHOLDER
Environment=MACHINE_ID=MACHINE_ID_PLACEHOLDER
Environment=AGENT_TOKEN=cloud-auto
Environment=RUST_LOG=info
ExecStart=/usr/local/bin/wisent-agent
Restart=always
[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable wisent-agent
systemctl start wisent-agent
"#;

pub async fn run(pool: PgPool, project: String, backend_url: String) {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    loop {
        interval.tick().await;
        if let Err(e) = tick(&pool, &project, &backend_url).await {
            tracing::error!("provisioner: {e}");
        }
    }
}

async fn tick(pool: &PgPool, project: &str, backend_url: &str) -> Result<(), String> {
    let unassigned: Vec<(Uuid, Option<String>, serde_json::Value)> = sqlx::query_as(
        "SELECT i.id, i.docker_image, i.docker_env FROM instances i
         LEFT JOIN machines m ON i.machine_id = m.id
         WHERE i.status = 'creating'
         AND (i.machine_id IS NULL OR m.status != 'online')
         AND i.created_at < now() - interval '10 seconds'")
        .fetch_all(pool).await.map_err(|e| e.to_string())?;

    // Track assignments this tick to avoid over-assigning before DB reflects changes
    let mut tick_assignments: std::collections::HashMap<Uuid, i32> = std::collections::HashMap::new();

    for (inst_id, _docker_image, docker_env) in &unassigned {
        // Find local machine with free GPU slots (accounting for this tick's assignments)
        let locals: Vec<(Uuid, Uuid, i32, i64)> = sqlx::query_as(
            "SELECT m.id, m.host_id, m.gpu_count,
             (SELECT COUNT(*) FROM instances i WHERE i.machine_id = m.id AND i.status IN ('creating', 'running'))
             FROM machines m
             WHERE m.is_cloud = false AND m.status = 'online' AND m.is_available = true
             ORDER BY m.price_per_hour_cents ASC")
            .fetch_all(pool).await.map_err(|e| e.to_string())?;

        let mut assigned_locally = false;
        for (mid, hid, gpu_count, db_count) in &locals {
            let tick_count = tick_assignments.get(mid).copied().unwrap_or(0);
            let total_used = *db_count as i32 + tick_count;
            if *gpu_count > total_used {
                sqlx::query("UPDATE instances SET machine_id = $1, host_id = $2 WHERE id = $3")
                    .bind(mid).bind(hid).bind(inst_id)
                    .execute(pool).await.ok();
                *tick_assignments.entry(*mid).or_insert(0) += 1;
                tracing::info!("Assigned {} to local machine {} (slot {}/{})", inst_id, mid, total_used + 1, gpu_count);
                assigned_locally = true;
                break;
            }
        }
        if assigned_locally { continue; }

        // No local GPU slots → provision cloud VM (one per job).
        // Caller can request a beefier GPU via docker_env.GPU_PREFERENCE
        // ("a100" maps to a2-highgpu-1g + nvidia-tesla-a100); default stays
        // n1-standard-4 + T4 for cheap inference jobs.
        let pref = docker_env
            .get("GPU_PREFERENCE")
            .and_then(|v| v.as_str())
            .unwrap_or("t4")
            .to_lowercase();
        let (machine_type, accel_type, gpu_model, gpu_vram_gb, ram_gb, price_cents) = match pref.as_str() {
            "a100" => ("a2-highgpu-1g", "nvidia-tesla-a100", "NVIDIA A100", 40, 85, 350),
            _      => ("n1-standard-4", "nvidia-tesla-t4",   "NVIDIA Tesla T4", 16, 32, 50),
        };

        let short_id = &inst_id.to_string()[..8];
        let vm_name = format!("wisent-cloud-{short_id}");

        // Purge any prior machine row for the same hostname so the cleanup
        // step can't race against this provisioning round (two rows sharing
        // a hostname caused us to delete the active VM in tick N+1).
        sqlx::query("DELETE FROM machines WHERE hostname = $1 AND id NOT IN (SELECT machine_id FROM instances WHERE machine_id IS NOT NULL AND status NOT IN ('destroyed','error'))")
            .bind(&vm_name).execute(pool).await.ok();

        let machine_id: Uuid = sqlx::query_scalar(
            "INSERT INTO machines (hostname, gpu_model, gpu_count, gpu_vram_gb, cpu_cores, ram_gb, disk_gb,
             price_per_hour_cents, status, is_available, is_cloud, cloud_instance_name,
             host_id, location_country)
             VALUES ($1, $2, 1, $3, 12, $4, 500, $5, 'pending', false, true, $1,
                     '00000000-0000-0000-0000-000000000001', 'US')
             RETURNING id")
            .bind(&vm_name).bind(gpu_model).bind(gpu_vram_gb).bind(ram_gb).bind(price_cents)
            .fetch_one(pool).await.map_err(|e| e.to_string())?;

        let script = STARTUP_TEMPLATE
            .replace("BACKEND_URL_PLACEHOLDER", backend_url)
            .replace("MACHINE_ID_PLACEHOLDER", &machine_id.to_string())
            .replace("AGENT_BINARY_URL", "https://storage.googleapis.com/wisent-compute/agent/wisent-agent-linux-amd64");

        match gcp::create_vm(project, machine_type, accel_type, &script, &vm_name).await {
            Ok((name, zone)) => {
                sqlx::query("UPDATE machines SET cloud_zone = $1, status = 'online' WHERE id = $2")
                    .bind(&zone).bind(machine_id).execute(pool).await.ok();
                sqlx::query("UPDATE instances SET machine_id = $1, host_id = '00000000-0000-0000-0000-000000000001' WHERE id = $2")
                    .bind(machine_id).bind(inst_id).execute(pool).await.ok();
                tracing::info!("Provisioned cloud VM {} for {}", name, inst_id);
            }
            Err(e) => {
                sqlx::query("DELETE FROM machines WHERE id = $1").bind(machine_id).execute(pool).await.ok();
                tracing::error!("Failed to provision VM for {}: {}", inst_id, e);
            }
        }
    }

    // Cleanup cloud VMs with no active instances. Guard against deleting a VM
    // whose hostname is still claimed by another active machine row (would
    // happen during a hostname-reuse race where the freshly-created VM gets
    // taken down). Also age-gate (>5 min) so we don't race the agent's first
    // heartbeat.
    let destroyed: Vec<(Uuid, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT m.id, m.cloud_instance_name, m.cloud_zone FROM machines m
         WHERE m.is_cloud = true AND m.status != 'offline'
         AND m.created_at < now() - interval '5 minutes'
         AND NOT EXISTS (SELECT 1 FROM instances i WHERE i.machine_id = m.id AND i.status NOT IN ('destroyed', 'error'))
         AND NOT EXISTS (SELECT 1 FROM machines m2 WHERE m2.hostname = m.hostname AND m2.id != m.id AND m2.status != 'offline')")
        .fetch_all(pool).await.map_err(|e| e.to_string())?;

    for (mid, name, zone) in &destroyed {
        if let (Some(n), Some(z)) = (name, zone) {
            gcp::delete_vm(project, n, z).await.ok();
        }
        sqlx::query("UPDATE machines SET status = 'offline' WHERE id = $1")
            .bind(mid).execute(pool).await.ok();
        tracing::info!("Cleaned up cloud machine {}", mid);
    }

    Ok(())
}
