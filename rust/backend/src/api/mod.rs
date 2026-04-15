use axum::{extract::{Path, Query, State}, http::StatusCode, Json};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::{db, models::*, AppState};

type S = Arc<AppState>;

// --- Health ---
pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok", "service": "compute.wisent.com"}))
}

// --- Auth ---
pub async fn auth_callback(State(s): State<S>, Json(req): Json<AuthCallbackReq>) -> Result<Json<Profile>, StatusCode> {
    db::upsert_profile(&s.pool, req.user_id, &req.email).await
        .map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn get_me(State(s): State<S>) -> Result<Json<Profile>, StatusCode> {
    // TODO: extract user_id from auth middleware
    Err(StatusCode::UNAUTHORIZED)
}

pub async fn update_me(State(s): State<S>, Json(req): Json<UpdateProfileReq>) -> Result<Json<Profile>, StatusCode> {
    Err(StatusCode::UNAUTHORIZED)
}

// --- Offers ---
#[derive(Deserialize)]
pub struct OfferQuery { limit: Option<i64>, offset: Option<i64> }

pub async fn list_offers(State(s): State<S>, Query(q): Query<OfferQuery>) -> Result<Json<Vec<Machine>>, StatusCode> {
    db::list_offers(&s.pool, q.limit.unwrap_or(50), q.offset.unwrap_or(0)).await
        .map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn get_offer(State(s): State<S>, Path(id): Path<Uuid>) -> Result<Json<Machine>, StatusCode> {
    db::get_machine(&s.pool, id).await
        .map(Json).map_err(|_| StatusCode::NOT_FOUND)
}

// --- Instances ---
pub async fn list_instances(State(s): State<S>) -> Result<Json<Vec<Instance>>, StatusCode> {
    Err(StatusCode::UNAUTHORIZED)
}

pub async fn create_instance(State(s): State<S>, Json(req): Json<CreateInstanceReq>) -> Result<(StatusCode, Json<Instance>), StatusCode> {
    Err(StatusCode::UNAUTHORIZED)
}

pub async fn get_instance(State(s): State<S>, Path(id): Path<Uuid>) -> Result<Json<Instance>, StatusCode> {
    db::get_instance(&s.pool, id).await
        .map(Json).map_err(|_| StatusCode::NOT_FOUND)
}

pub async fn start_instance(State(s): State<S>, Path(id): Path<Uuid>) -> Result<Json<serde_json::Value>, StatusCode> {
    db::update_instance_status(&s.pool, id, "creating").await
        .map(|_| Json(serde_json::json!({"status": "starting"})))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn stop_instance(State(s): State<S>, Path(id): Path<Uuid>) -> Result<Json<serde_json::Value>, StatusCode> {
    db::update_instance_status(&s.pool, id, "stopping").await
        .map(|_| Json(serde_json::json!({"status": "stopping"})))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn destroy_instance(State(s): State<S>, Path(id): Path<Uuid>) -> Result<Json<serde_json::Value>, StatusCode> {
    db::update_instance_status(&s.pool, id, "destroying").await
        .map(|_| Json(serde_json::json!({"status": "destroying"})))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

// --- Billing ---
pub async fn get_balance(State(s): State<S>) -> Result<Json<serde_json::Value>, StatusCode> {
    Err(StatusCode::UNAUTHORIZED)
}

pub async fn list_transactions(State(s): State<S>) -> Result<Json<Vec<CreditTransaction>>, StatusCode> {
    Err(StatusCode::UNAUTHORIZED)
}

pub async fn checkout(State(s): State<S>, Json(req): Json<CheckoutReq>) -> Result<Json<CheckoutResp>, StatusCode> {
    Err(StatusCode::NOT_IMPLEMENTED)
}

pub async fn stripe_webhook(State(s): State<S>, body: String) -> StatusCode {
    tracing::info!("Stripe webhook received");
    StatusCode::OK
}

// --- API Keys ---
pub async fn list_api_keys(State(s): State<S>) -> Result<Json<Vec<ApiKey>>, StatusCode> {
    Err(StatusCode::UNAUTHORIZED)
}

pub async fn create_api_key(State(s): State<S>, Json(req): Json<CreateApiKeyReq>) -> Result<Json<CreateApiKeyResp>, StatusCode> {
    Err(StatusCode::UNAUTHORIZED)
}

pub async fn revoke_api_key(State(s): State<S>, Path(id): Path<Uuid>) -> StatusCode {
    StatusCode::UNAUTHORIZED
}

// --- Host ---
pub async fn list_host_machines(State(s): State<S>) -> Result<Json<Vec<Machine>>, StatusCode> {
    Err(StatusCode::UNAUTHORIZED)
}

pub async fn register_machine(State(s): State<S>, Json(req): Json<RegisterMachineReq>) -> Result<Json<RegisterMachineResp>, StatusCode> {
    Err(StatusCode::UNAUTHORIZED)
}

pub async fn get_host_machine(State(s): State<S>, Path(id): Path<Uuid>) -> Result<Json<Machine>, StatusCode> {
    db::get_machine(&s.pool, id).await
        .map(Json).map_err(|_| StatusCode::NOT_FOUND)
}

pub async fn delete_machine(State(s): State<S>, Path(id): Path<Uuid>) -> StatusCode {
    StatusCode::UNAUTHORIZED
}

pub async fn get_earnings(State(s): State<S>) -> Result<Json<EarningsSummary>, StatusCode> {
    Err(StatusCode::UNAUTHORIZED)
}

// --- Agent ---
pub async fn agent_heartbeat(State(s): State<S>, Json(req): Json<MachineHeartbeat>) -> StatusCode {
    match db::update_heartbeat(&s.pool, req.machine_id, &req.agent_version).await {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
