mod api;
mod auth;
mod db;
mod models;
mod scheduler;

use axum::{Router, middleware as axum_mw};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

pub struct AppState {
    pub pool: sqlx::PgPool,
    pub jwt_secret: String,
    pub stripe_secret: String,
    pub stripe_webhook_secret: String,
    pub cors_origins: Vec<String>,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .init();

    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL required");
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".into());
    let jwt_secret = std::env::var("SUPABASE_JWT_SECRET").unwrap_or_default();

    let pool = PgPoolOptions::new()
        .max_connections(20)
        .connect(&db_url)
        .await
        .expect("Failed to connect to database");

    tracing::info!("Connected to database");

    let state = Arc::new(AppState {
        pool: pool.clone(),
        jwt_secret,
        stripe_secret: std::env::var("STRIPE_SECRET_KEY").unwrap_or_default(),
        stripe_webhook_secret: std::env::var("STRIPE_WEBHOOK_SECRET").unwrap_or_default(),
        cors_origins: std::env::var("CORS_ORIGINS")
            .unwrap_or_else(|_| "http://localhost:3000".into())
            .split(',')
            .map(|s| s.trim().to_string())
            .collect(),
    });

    // Background schedulers
    let billing_pool = pool.clone();
    tokio::spawn(async move {
        scheduler::billing_ticker(billing_pool).await;
    });
    let stale_pool = pool.clone();
    tokio::spawn(async move {
        scheduler::stale_checker(stale_pool).await;
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        // Public
        .route("/healthz", axum::routing::get(api::health))
        .route("/api/v1/offers", axum::routing::get(api::list_offers))
        .route("/api/v1/offers/{id}", axum::routing::get(api::get_offer))
        .route("/api/v1/auth/callback", axum::routing::post(api::auth_callback))
        // Webhooks
        .route("/api/v1/billing/webhook/stripe", axum::routing::post(api::stripe_webhook))
        // Agent
        .route("/api/v1/agent/heartbeat", axum::routing::post(api::agent_heartbeat))
        .route("/api/v1/agent/commands", axum::routing::get(api::agent::commands))
        .route("/api/v1/agent/commands/{id}/ack", axum::routing::post(api::agent::command_ack))
        // Authenticated
        .route("/api/v1/users/me", axum::routing::get(api::get_me).put(api::update_me))
        .route("/api/v1/instances", axum::routing::get(api::list_instances).post(api::create_instance))
        .route("/api/v1/instances/{id}", axum::routing::get(api::get_instance).delete(api::destroy_instance))
        .route("/api/v1/instances/{id}/start", axum::routing::post(api::start_instance))
        .route("/api/v1/instances/{id}/stop", axum::routing::post(api::stop_instance))
        .route("/api/v1/billing/balance", axum::routing::get(api::get_balance))
        .route("/api/v1/billing/transactions", axum::routing::get(api::list_transactions))
        .route("/api/v1/billing/checkout", axum::routing::post(api::checkout))
        .route("/api/v1/api-keys", axum::routing::get(api::list_api_keys).post(api::create_api_key))
        .route("/api/v1/api-keys/{id}", axum::routing::delete(api::revoke_api_key))
        // Host
        .route("/api/v1/host/machines", axum::routing::get(api::list_host_machines).post(api::register_machine))
        .route("/api/v1/host/machines/{id}", axum::routing::get(api::get_host_machine).delete(api::delete_machine))
        .route("/api/v1/host/earnings", axum::routing::get(api::get_earnings))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("Failed to bind");

    tracing::info!("Server listening on :{port}");
    axum::serve(listener, app).await.unwrap();
}
