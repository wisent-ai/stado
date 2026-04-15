use axum::{extract::{Path, Query, State}, http::StatusCode, Json};
use rand::Rng;
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::{auth::AuthUser, db, models::*, AppState};

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

pub async fn get_me(State(s): State<S>, user: AuthUser) -> Result<Json<Profile>, StatusCode> {
    db::get_profile(&s.pool, user.id).await
        .map(Json).map_err(|_| StatusCode::NOT_FOUND)
}

pub async fn update_me(State(s): State<S>, user: AuthUser, Json(req): Json<UpdateProfileReq>) -> Result<Json<Profile>, StatusCode> {
    db::update_profile(&s.pool, user.id, &req).await
        .map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

// --- Offers ---
#[derive(Deserialize)]
pub struct OfferQuery { limit: Option<i64>, offset: Option<i64> }

pub async fn list_offers(State(s): State<S>, Query(q): Query<OfferQuery>) -> Result<Json<Vec<Machine>>, StatusCode> {
    db::list_offers(&s.pool, q.limit.unwrap_or(50), q.offset.unwrap_or(0)).await
        .map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn get_offer(State(s): State<S>, Path(id): Path<Uuid>) -> Result<Json<Machine>, StatusCode> {
    db::get_machine(&s.pool, id).await.map(Json).map_err(|_| StatusCode::NOT_FOUND)
}

// --- Instances ---
pub async fn list_instances(State(s): State<S>, user: AuthUser) -> Result<Json<Vec<Instance>>, StatusCode> {
    db::list_instances(&s.pool, user.id).await
        .map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn create_instance(State(s): State<S>, user: AuthUser, Json(req): Json<CreateInstanceReq>) -> Result<(StatusCode, Json<Instance>), StatusCode> {
    let machine = db::get_machine(&s.pool, req.machine_id).await.map_err(|_| StatusCode::NOT_FOUND)?;
    if !machine.is_available || machine.status != "online" {
        return Err(StatusCode::CONFLICT);
    }
    let balance = db::get_balance(&s.pool, user.id).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if balance < machine.price_per_hour_cents as i64 {
        return Err(StatusCode::PAYMENT_REQUIRED);
    }
    let inst = db::create_instance(&s.pool, user.id, &machine, &req).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((StatusCode::CREATED, Json(inst)))
}

pub async fn get_instance(State(s): State<S>, user: AuthUser, Path(id): Path<Uuid>) -> Result<Json<Instance>, StatusCode> {
    let inst = db::get_instance(&s.pool, id).await.map_err(|_| StatusCode::NOT_FOUND)?;
    if inst.user_id != user.id { return Err(StatusCode::FORBIDDEN); }
    Ok(Json(inst))
}

pub async fn start_instance(State(s): State<S>, user: AuthUser, Path(id): Path<Uuid>) -> Result<Json<serde_json::Value>, StatusCode> {
    let inst = db::get_instance(&s.pool, id).await.map_err(|_| StatusCode::NOT_FOUND)?;
    if inst.user_id != user.id { return Err(StatusCode::FORBIDDEN); }
    db::update_instance_status(&s.pool, id, "creating").await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({"status": "starting"})))
}

pub async fn stop_instance(State(s): State<S>, user: AuthUser, Path(id): Path<Uuid>) -> Result<Json<serde_json::Value>, StatusCode> {
    let inst = db::get_instance(&s.pool, id).await.map_err(|_| StatusCode::NOT_FOUND)?;
    if inst.user_id != user.id { return Err(StatusCode::FORBIDDEN); }
    db::update_instance_status(&s.pool, id, "stopping").await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({"status": "stopping"})))
}

pub async fn destroy_instance(State(s): State<S>, user: AuthUser, Path(id): Path<Uuid>) -> Result<Json<serde_json::Value>, StatusCode> {
    let inst = db::get_instance(&s.pool, id).await.map_err(|_| StatusCode::NOT_FOUND)?;
    if inst.user_id != user.id { return Err(StatusCode::FORBIDDEN); }
    db::update_instance_status(&s.pool, id, "destroying").await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({"status": "destroying"})))
}

// --- Billing ---
pub async fn get_balance(State(s): State<S>, user: AuthUser) -> Result<Json<serde_json::Value>, StatusCode> {
    let bal = db::get_balance(&s.pool, user.id).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({"credit_balance_cents": bal})))
}

pub async fn list_transactions(State(s): State<S>, user: AuthUser) -> Result<Json<Vec<CreditTransaction>>, StatusCode> {
    db::list_credit_transactions(&s.pool, user.id, 50).await
        .map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn checkout(State(s): State<S>, user: AuthUser, Json(req): Json<CheckoutReq>) -> Result<Json<CheckoutResp>, StatusCode> {
    // TODO: create Stripe checkout session
    Err(StatusCode::NOT_IMPLEMENTED)
}

pub async fn stripe_webhook(State(s): State<S>, body: String) -> StatusCode {
    // TODO: verify signature, handle checkout.session.completed
    tracing::info!("Stripe webhook received");
    StatusCode::OK
}

// --- API Keys ---
pub async fn list_api_keys(State(s): State<S>, user: AuthUser) -> Result<Json<Vec<ApiKey>>, StatusCode> {
    db::list_api_keys(&s.pool, user.id).await
        .map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn create_api_key(State(s): State<S>, user: AuthUser, Json(req): Json<CreateApiKeyReq>) -> Result<Json<CreateApiKeyResp>, StatusCode> {
    let raw_key: String = format!("wsk_{}", hex::encode(rand::rng().random::<[u8; 32]>()));
    let prefix = raw_key[..12].to_string();
    let hash = bcrypt::hash(&raw_key, 10).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let key = db::create_api_key(&s.pool, user.id, &req.name, &prefix, &hash).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(CreateApiKeyResp { id: key.id, name: key.name, key: raw_key, key_prefix: prefix }))
}

pub async fn revoke_api_key(State(s): State<S>, user: AuthUser, Path(id): Path<Uuid>) -> StatusCode {
    match db::revoke_api_key(&s.pool, id, user.id).await {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

// --- Host ---
pub async fn list_host_machines(State(s): State<S>, user: AuthUser) -> Result<Json<Vec<Machine>>, StatusCode> {
    db::list_host_machines(&s.pool, user.id).await
        .map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn register_machine(State(s): State<S>, user: AuthUser, Json(req): Json<RegisterMachineReq>) -> Result<Json<RegisterMachineResp>, StatusCode> {
    let raw_token = format!("wat_{}", hex::encode(rand::rng().random::<[u8; 32]>()));
    let hash = bcrypt::hash(&raw_token, 10).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let machine = db::register_machine(&s.pool, user.id, &req, &hash).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(RegisterMachineResp { id: machine.id, agent_token: raw_token }))
}

pub async fn get_host_machine(State(s): State<S>, user: AuthUser, Path(id): Path<Uuid>) -> Result<Json<Machine>, StatusCode> {
    let m = db::get_machine(&s.pool, id).await.map_err(|_| StatusCode::NOT_FOUND)?;
    if m.host_id != user.id { return Err(StatusCode::FORBIDDEN); }
    Ok(Json(m))
}

pub async fn delete_machine(State(s): State<S>, user: AuthUser, Path(id): Path<Uuid>) -> StatusCode {
    match db::delete_machine(&s.pool, id, user.id).await {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn get_earnings(State(s): State<S>, user: AuthUser) -> Result<Json<EarningsSummary>, StatusCode> {
    db::get_earnings_summary(&s.pool, user.id).await
        .map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

// --- Agent ---
pub async fn agent_heartbeat(State(s): State<S>, Json(req): Json<MachineHeartbeat>) -> StatusCode {
    match db::update_heartbeat(&s.pool, req.machine_id, &req.agent_version).await {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
