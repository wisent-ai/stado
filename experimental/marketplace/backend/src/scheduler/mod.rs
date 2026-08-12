pub mod provisioner;

use sqlx::PgPool;
use std::time::Duration;

use crate::db;

pub async fn billing_ticker(pool: PgPool) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;

        // Get all running instances
        let instances: Vec<(uuid::Uuid, uuid::Uuid, uuid::Uuid, i32, Option<chrono::DateTime<chrono::Utc>>)> =
            match sqlx::query_as(
                "SELECT id, user_id, host_id, price_per_hour_cents, last_billed_at
                 FROM instances WHERE status = 'running'")
                .fetch_all(&pool).await
            {
                Ok(rows) => rows,
                Err(e) => { tracing::error!("billing query: {e}"); continue; }
            };

        for (inst_id, user_id, host_id, price, last_billed) in &instances {
            let should_bill = match last_billed {
                Some(t) => chrono::Utc::now().signed_duration_since(*t).num_seconds() >= 3600,
                None => false,
            };
            if !should_bill { continue; }

            // Deduct credits
            let result = sqlx::query_scalar::<_, bool>(
                "SELECT deduct_credits($1, $2, 'rental_charge', $3)")
                .bind(user_id).bind(*price as i64).bind(inst_id)
                .fetch_one(&pool).await;

            match result {
                Ok(true) => {
                    // Update billing timestamp
                    sqlx::query("UPDATE instances SET last_billed_at = now(), total_cost_cents = total_cost_cents + $1 WHERE id = $2")
                        .bind(*price as i64).bind(inst_id).execute(&pool).await.ok();

                    // Record host earning (15% platform fee)
                    let fee = (*price as i64) * 15 / 100;
                    let net = *price as i64 - fee;
                    sqlx::query(
                        "INSERT INTO host_earnings (host_id, instance_id, amount_cents, platform_fee_cents, net_amount_cents)
                         VALUES ($1, $2, $3, $4, $5)")
                        .bind(host_id).bind(inst_id).bind(*price as i64).bind(fee).bind(net)
                        .execute(&pool).await.ok();

                    tracing::info!("billed instance {} : {}¢", inst_id, price);
                }
                Ok(false) => {
                    // Insufficient balance — auto-stop
                    sqlx::query("UPDATE instances SET status = 'stopping' WHERE id = $1")
                        .bind(inst_id).execute(&pool).await.ok();
                    tracing::warn!("auto-stopping {} (insufficient balance)", inst_id);
                }
                Err(e) => tracing::error!("billing deduct error: {e}"),
            }
        }
    }
}

pub async fn stale_checker(pool: PgPool) {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    loop {
        interval.tick().await;
        match db::mark_stale(&pool, 120).await {
            Ok(n) if n > 0 => tracing::info!("marked {n} machines stale"),
            Ok(_) => {}
            Err(e) => tracing::error!("stale check: {e}"),
        }
    }
}
