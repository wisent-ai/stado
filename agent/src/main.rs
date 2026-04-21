mod container;
mod heartbeat;
mod vast;

use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .init();

    let server_url = std::env::var("BACKEND_URL").unwrap_or_else(|_| "http://localhost:8080".into());
    let machine_id = std::env::var("MACHINE_ID").expect("MACHINE_ID required");
    let agent_token = std::env::var("AGENT_TOKEN").unwrap_or_default();

    tracing::info!("Agent starting: machine_id={machine_id}");

    let client = reqwest::Client::new();
    let docker = std::sync::Arc::new(
        bollard::Docker::connect_with_local_defaults()
            .expect("Failed to connect to Docker"),
    );

    // Heartbeat loop
    let hb_client = client.clone();
    let hb_url = server_url.clone();
    let hb_mid = machine_id.clone();
    let hb_token = agent_token.clone();
    tokio::spawn(async move {
        heartbeat::run(&hb_client, &hb_url, &hb_mid, &hb_token).await;
    });

    // Command poll loop
    let cmd_client = client.clone();
    let cmd_url = server_url.clone();
    let cmd_mid = machine_id.clone();
    let cmd_token = agent_token.clone();
    let cmd_docker = docker.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            tracing::info!("polling commands...");
            match container::poll_commands(&cmd_client, &*cmd_docker, &cmd_url, &cmd_mid, &cmd_token).await {
                Ok(()) => {}
                Err(e) => tracing::error!("command poll error: {e}"),
            }
        }
    });

    // Wait for signal
    tokio::signal::ctrl_c().await.ok();
    tracing::info!("Agent shutting down");
}
