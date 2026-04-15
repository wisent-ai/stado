use sqlx::PgPool;
use std::time::Duration;

use crate::db;

pub async fn billing_ticker(pool: PgPool) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;
        // List running instances, deduct credits, record earnings
        // TODO: implement billing logic
        tracing::debug!("billing tick");
    }
}

pub async fn stale_checker(pool: PgPool) {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    loop {
        interval.tick().await;
        match db::mark_stale(&pool, 120).await {
            Ok(n) if n > 0 => tracing::info!("marked {n} machines stale"),
            Ok(_) => {}
            Err(e) => tracing::error!("stale check failed: {e}"),
        }
    }
}
