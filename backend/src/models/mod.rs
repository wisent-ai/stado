use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

// --- User ---

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Profile {
    pub id: Uuid,
    pub email: Option<String>,
    pub full_name: Option<String>,
    pub avatar_url: Option<String>,
    pub role: String,
    pub stripe_customer_id: Option<String>,
    pub credit_balance_cents: i64,
    pub is_host: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct AuthCallbackReq {
    pub user_id: Uuid,
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProfileReq {
    pub full_name: Option<String>,
    pub avatar_url: Option<String>,
}

// --- API Key ---

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ApiKey {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub key_prefix: String,
    pub key_hash: String,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateApiKeyReq {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct CreateApiKeyResp {
    pub id: Uuid,
    pub name: String,
    pub key: String,
    pub key_prefix: String,
}

// --- Machine ---

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Machine {
    pub id: Uuid,
    pub host_id: Uuid,
    pub hostname: String,
    pub label: Option<String>,
    pub status: String,
    pub gpu_model: Option<String>,
    pub gpu_count: i32,
    pub gpu_vram_gb: i32,
    pub cpu_model: Option<String>,
    pub cpu_cores: i32,
    pub ram_gb: i32,
    pub disk_gb: i32,
    pub location_country: Option<String>,
    pub location_region: Option<String>,
    pub price_per_hour_cents: i32,
    pub agent_version: Option<String>,
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub is_available: bool,
    pub current_instance_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct RegisterMachineReq {
    pub hostname: String,
    pub label: Option<String>,
    pub gpu_model: String,
    pub gpu_count: i32,
    pub gpu_vram_gb: i32,
    pub cpu_model: String,
    pub cpu_cores: i32,
    pub ram_gb: i32,
    pub disk_gb: i32,
    pub location_country: String,
    pub location_region: String,
    pub price_per_hour_cents: i32,
}

#[derive(Debug, Serialize)]
pub struct RegisterMachineResp {
    pub id: Uuid,
    pub agent_token: String,
}

#[derive(Debug, Deserialize)]
pub struct MachineHeartbeat {
    pub machine_id: Uuid,
    pub agent_version: String,
    pub gpu_utilization: f32,
    pub gpu_temp: f32,
    pub gpu_memory_used_mb: i64,
    pub gpu_memory_total_mb: i64,
    pub cpu_utilization: f32,
    pub ram_used_mb: i64,
    pub ram_total_mb: i64,
}

// --- Instance ---

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Instance {
    pub id: Uuid,
    pub user_id: Uuid,
    pub machine_id: Uuid,
    pub host_id: Uuid,
    pub status: String,
    pub docker_image: String,
    pub docker_env: serde_json::Value,
    pub docker_ports: serde_json::Value,
    pub docker_cmd: Option<String>,
    pub disk_gb: i32,
    pub ssh_host: Option<String>,
    pub ssh_port: Option<i32>,
    pub ssh_public_key: Option<String>,
    pub jupyter_url: Option<String>,
    pub jupyter_token: Option<String>,
    pub price_per_hour_cents: i32,
    pub total_cost_cents: i64,
    pub started_at: Option<DateTime<Utc>>,
    pub stopped_at: Option<DateTime<Utc>>,
    pub destroyed_at: Option<DateTime<Utc>>,
    pub last_billed_at: Option<DateTime<Utc>>,
    pub gpu_utilization: f32,
    pub gpu_memory_used_mb: i32,
    pub label: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateInstanceReq {
    pub machine_id: Option<Uuid>,
    pub docker_image: String,
    pub docker_env: Option<serde_json::Value>,
    pub docker_ports: Option<serde_json::Value>,
    pub docker_cmd: Option<String>,
    pub disk_gb: Option<i32>,
    pub ssh_public_key: String,
    pub label: Option<String>,
}

// --- Billing ---

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct CreditTransaction {
    pub id: Uuid,
    pub user_id: Uuid,
    pub amount_cents: i64,
    #[sqlx(rename = "type")]
    pub tx_type: String,
    pub description: Option<String>,
    pub instance_id: Option<Uuid>,
    pub balance_after_cents: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreditPackage {
    pub id: &'static str,
    pub name: &'static str,
    pub credits_cents: i64,
    pub price_cents: i64,
    pub bonus_percent: i32,
}

pub const CREDIT_PACKAGES: &[CreditPackage] = &[
    CreditPackage { id: "starter", name: "Starter", credits_cents: 500, price_cents: 500, bonus_percent: 0 },
    CreditPackage { id: "builder", name: "Builder", credits_cents: 2200, price_cents: 2000, bonus_percent: 10 },
    CreditPackage { id: "pro", name: "Pro", credits_cents: 6000, price_cents: 5000, bonus_percent: 20 },
    CreditPackage { id: "enterprise", name: "Enterprise", credits_cents: 15000, price_cents: 10000, bonus_percent: 50 },
];

pub fn get_package(id: &str) -> Option<&'static CreditPackage> {
    CREDIT_PACKAGES.iter().find(|p| p.id == id)
}

#[derive(Debug, Deserialize)]
pub struct CheckoutReq {
    pub package_id: String,
}

#[derive(Debug, Serialize)]
pub struct CheckoutResp {
    pub checkout_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct HostEarning {
    pub id: Uuid,
    pub host_id: Uuid,
    pub instance_id: Uuid,
    pub amount_cents: i64,
    pub platform_fee_cents: i64,
    pub net_amount_cents: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct EarningsSummary {
    pub total_earned_cents: i64,
    pub pending_payout_cents: i64,
    pub total_paid_out_cents: i64,
    pub this_month_cents: i64,
}
