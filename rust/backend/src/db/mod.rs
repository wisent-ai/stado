use sqlx::PgPool;
use uuid::Uuid;

use crate::models::*;

// --- Users ---

pub async fn get_profile(pool: &PgPool, id: Uuid) -> sqlx::Result<Profile> {
    sqlx::query_as("SELECT * FROM profiles WHERE id = $1")
        .bind(id).fetch_one(pool).await
}

pub async fn upsert_profile(pool: &PgPool, id: Uuid, email: &str) -> sqlx::Result<Profile> {
    sqlx::query_as(
        "INSERT INTO profiles (id, email) VALUES ($1, $2)
         ON CONFLICT (id) DO UPDATE SET email = $2, updated_at = now()
         RETURNING *")
        .bind(id).bind(email).fetch_one(pool).await
}

pub async fn update_profile(pool: &PgPool, id: Uuid, req: &UpdateProfileReq) -> sqlx::Result<Profile> {
    sqlx::query_as(
        "UPDATE profiles SET full_name = COALESCE($2, full_name),
         avatar_url = COALESCE($3, avatar_url), updated_at = now()
         WHERE id = $1 RETURNING *")
        .bind(id).bind(&req.full_name).bind(&req.avatar_url)
        .fetch_one(pool).await
}

// --- API Keys ---

pub async fn list_api_keys(pool: &PgPool, user_id: Uuid) -> sqlx::Result<Vec<ApiKey>> {
    sqlx::query_as("SELECT * FROM api_keys WHERE user_id = $1 AND revoked_at IS NULL ORDER BY created_at DESC")
        .bind(user_id).fetch_all(pool).await
}

pub async fn create_api_key(pool: &PgPool, user_id: Uuid, name: &str, prefix: &str, hash: &str) -> sqlx::Result<ApiKey> {
    sqlx::query_as(
        "INSERT INTO api_keys (user_id, name, key_prefix, key_hash) VALUES ($1, $2, $3, $4) RETURNING *")
        .bind(user_id).bind(name).bind(prefix).bind(hash)
        .fetch_one(pool).await
}

pub async fn revoke_api_key(pool: &PgPool, id: Uuid, user_id: Uuid) -> sqlx::Result<()> {
    sqlx::query("UPDATE api_keys SET revoked_at = now() WHERE id = $1 AND user_id = $2")
        .bind(id).bind(user_id).execute(pool).await?;
    Ok(())
}

// --- Machines ---

pub async fn list_offers(pool: &PgPool, limit: i64, offset: i64) -> sqlx::Result<Vec<Machine>> {
    sqlx::query_as(
        "SELECT * FROM machines WHERE status = 'online' AND is_available = true
         ORDER BY price_per_hour_cents ASC LIMIT $1 OFFSET $2")
        .bind(limit).bind(offset).fetch_all(pool).await
}

pub async fn get_machine(pool: &PgPool, id: Uuid) -> sqlx::Result<Machine> {
    sqlx::query_as("SELECT * FROM machines WHERE id = $1")
        .bind(id).fetch_one(pool).await
}

pub async fn list_host_machines(pool: &PgPool, host_id: Uuid) -> sqlx::Result<Vec<Machine>> {
    sqlx::query_as("SELECT * FROM machines WHERE host_id = $1 ORDER BY created_at DESC")
        .bind(host_id).fetch_all(pool).await
}

pub async fn register_machine(pool: &PgPool, host_id: Uuid, req: &RegisterMachineReq, token_hash: &str) -> sqlx::Result<Machine> {
    sqlx::query("UPDATE profiles SET is_host = true, role = 'host' WHERE id = $1")
        .bind(host_id).execute(pool).await?;
    sqlx::query_as(
        "INSERT INTO machines (host_id, hostname, label, gpu_model, gpu_count, gpu_vram_gb,
         cpu_model, cpu_cores, ram_gb, disk_gb, location_country, location_region,
         price_per_hour_cents, status, is_available, token_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,'online',true,$14) RETURNING *")
        .bind(host_id).bind(&req.hostname).bind(&req.label)
        .bind(&req.gpu_model).bind(req.gpu_count).bind(req.gpu_vram_gb)
        .bind(&req.cpu_model).bind(req.cpu_cores).bind(req.ram_gb).bind(req.disk_gb)
        .bind(&req.location_country).bind(&req.location_region)
        .bind(req.price_per_hour_cents).bind(token_hash)
        .fetch_one(pool).await
}

pub async fn update_heartbeat(pool: &PgPool, machine_id: Uuid, agent_version: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE machines SET status = 'online', last_heartbeat = now(), agent_version = $2 WHERE id = $1")
        .bind(machine_id).bind(agent_version).execute(pool).await?;
    Ok(())
}

pub async fn delete_machine(pool: &PgPool, id: Uuid, host_id: Uuid) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM machines WHERE id = $1 AND host_id = $2")
        .bind(id).bind(host_id).execute(pool).await?;
    Ok(())
}

pub async fn mark_stale(pool: &PgPool, stale_secs: i64) -> sqlx::Result<u64> {
    let r = sqlx::query(
        "UPDATE machines SET status = 'offline' WHERE status = 'online'
         AND last_heartbeat < now() - make_interval(secs => $1)")
        .bind(stale_secs as f64).execute(pool).await?;
    Ok(r.rows_affected())
}

// --- Instances ---

pub async fn create_instance(pool: &PgPool, user_id: Uuid, machine: &Machine, req: &CreateInstanceReq) -> sqlx::Result<Instance> {
    let disk = req.disk_gb.unwrap_or(20);
    let env = req.docker_env.clone().unwrap_or(serde_json::json!({}));
    let ports = req.docker_ports.clone().unwrap_or(serde_json::json!({}));
    sqlx::query_as(
        "INSERT INTO instances (user_id, machine_id, host_id, docker_image, docker_env,
         docker_ports, docker_cmd, disk_gb, ssh_public_key, label, price_per_hour_cents, status)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'creating') RETURNING *")
        .bind(user_id).bind(machine.id).bind(machine.host_id)
        .bind(&req.docker_image).bind(&env).bind(&ports)
        .bind(&req.docker_cmd).bind(disk).bind(&req.ssh_public_key)
        .bind(&req.label).bind(machine.price_per_hour_cents)
        .fetch_one(pool).await
}

pub async fn list_instances(pool: &PgPool, user_id: Uuid) -> sqlx::Result<Vec<Instance>> {
    sqlx::query_as("SELECT * FROM instances WHERE user_id = $1 ORDER BY created_at DESC")
        .bind(user_id).fetch_all(pool).await
}

pub async fn get_instance(pool: &PgPool, id: Uuid) -> sqlx::Result<Instance> {
    sqlx::query_as("SELECT * FROM instances WHERE id = $1")
        .bind(id).fetch_one(pool).await
}

pub async fn update_instance_status(pool: &PgPool, id: Uuid, status: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE instances SET status = $2 WHERE id = $1")
        .bind(id).bind(status).execute(pool).await?;
    Ok(())
}

// --- Billing ---

pub async fn get_balance(pool: &PgPool, user_id: Uuid) -> sqlx::Result<i64> {
    let row: (i64,) = sqlx::query_as("SELECT credit_balance_cents FROM profiles WHERE id = $1")
        .bind(user_id).fetch_one(pool).await?;
    Ok(row.0)
}

pub async fn list_credit_transactions(pool: &PgPool, user_id: Uuid, limit: i64) -> sqlx::Result<Vec<CreditTransaction>> {
    sqlx::query_as(
        "SELECT * FROM credit_transactions WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2")
        .bind(user_id).bind(limit).fetch_all(pool).await
}

pub async fn get_earnings_summary(pool: &PgPool, host_id: Uuid) -> sqlx::Result<EarningsSummary> {
    let total: (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(net_amount_cents), 0) FROM host_earnings WHERE host_id = $1")
        .bind(host_id).fetch_one(pool).await?;
    let month: (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(net_amount_cents), 0) FROM host_earnings
         WHERE host_id = $1 AND created_at >= date_trunc('month', now())")
        .bind(host_id).fetch_one(pool).await?;
    let paid: (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(amount_cents), 0) FROM payouts
         WHERE host_id = $1 AND status = 'completed'")
        .bind(host_id).fetch_one(pool).await?;

    Ok(EarningsSummary {
        total_earned_cents: total.0,
        pending_payout_cents: total.0 - paid.0,
        total_paid_out_cents: paid.0,
        this_month_cents: month.0,
    })
}
