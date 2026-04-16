use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

use crate::gcp;

const STARTUP_TEMPLATE: &str = r#"#!/bin/bash
set -euxo pipefail
exec > /var/log/wisent-agent-setup.log 2>&1

apt-get update && apt-get install -y docker.io curl
distribution=$(. /etc/os-release;echo $ID$VERSION_ID)
curl -fsSL https://nvidia.github.io/libnvidia-container/gpgkey | gpg --dearmor -o /usr/share/keyrings/nvidia-container-toolkit-keyring.gpg
curl -s -L https://nvidia.github.io/libnvidia-container/$distribution/libnvidia-container.list | \
  sed 's#deb https://#deb [signed-by=/usr/share/keyrings/nvidia-container-toolkit-keyring.gpg] https://#g' | \
  tee /etc/apt/sources.list.d/nvidia-container-toolkit.list
apt-get update && apt-get install -y nvidia-container-toolkit
nvidia-ctk runtime configure --runtime=docker && systemctl restart docker

curl -sL "AGENT_BINARY_URL" -o /usr/local/bin/wisent-agent && chmod +x /usr/local/bin/wisent-agent
export BACKEND_URL="BACKEND_URL_PLACEHOLDER"
export MACHINE_ID="MACHINE_ID_PLACEHOLDER"
export AGENT_TOKEN="cloud-auto"
export RUST_LOG=info
nohup /usr/local/bin/wisent-agent > /var/log/wisent-agent.log 2>&1 &
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
    let unassigned: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT i.id, i.docker_image FROM instances i
         LEFT JOIN machines m ON i.machine_id = m.id
         WHERE i.status = 'creating'
         AND (i.machine_id IS NULL OR m.status != 'online')
         AND i.created_at < now() - interval '10 seconds'")
        .fetch_all(pool).await.map_err(|e| e.to_string())?;

    for (inst_id, _docker_image) in &unassigned {
        // Find local machine with free GPU slots
        // Free slots = gpu_count - count of running/creating instances on that machine
        let local: Option<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT m.id, m.host_id FROM machines m
             WHERE m.is_cloud = false AND m.status = 'online' AND m.is_available = true
             AND m.gpu_count > (
                 SELECT COUNT(*) FROM instances i
                 WHERE i.machine_id = m.id AND i.status IN ('creating', 'running')
             )
             ORDER BY m.price_per_hour_cents ASC LIMIT 1")
            .fetch_optional(pool).await.map_err(|e| e.to_string())?;

        if let Some((machine_id, host_id)) = local {
            sqlx::query("UPDATE instances SET machine_id = $1, host_id = $2 WHERE id = $3")
                .bind(machine_id).bind(host_id).bind(inst_id)
                .execute(pool).await.ok();
            tracing::info!("Assigned {} to local machine {}", inst_id, machine_id);
            continue;
        }

        // No local GPU slots → provision cloud VM (one per job)
        let short_id = &inst_id.to_string()[..8];
        let vm_name = format!("wisent-cloud-{short_id}");
        let machine_type = "n1-standard-4";
        let accel_type = "nvidia-tesla-t4";

        let machine_id: Uuid = sqlx::query_scalar(
            "INSERT INTO machines (hostname, gpu_model, gpu_count, gpu_vram_gb, cpu_cores, ram_gb, disk_gb,
             price_per_hour_cents, status, is_available, is_cloud, cloud_instance_name,
             host_id, location_country)
             VALUES ($1, 'NVIDIA Tesla T4', 1, 16, 4, 32, 200, 50, 'pending', false, true, $1,
                     '00000000-0000-0000-0000-000000000001', 'US')
             RETURNING id")
            .bind(&vm_name)
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

    // Cleanup cloud VMs with no active instances
    let destroyed: Vec<(Uuid, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT m.id, m.cloud_instance_name, m.cloud_zone FROM machines m
         WHERE m.is_cloud = true AND m.status != 'offline'
         AND NOT EXISTS (SELECT 1 FROM instances i WHERE i.machine_id = m.id AND i.status NOT IN ('destroyed', 'error'))")
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
