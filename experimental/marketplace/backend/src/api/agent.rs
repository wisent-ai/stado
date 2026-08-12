use axum::{extract::{Path, Query, State}, http::StatusCode, Json};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::{models::*, AppState};

type S = Arc<AppState>;

#[derive(Deserialize)]
pub struct CommandsQuery {
    machine_id: Option<String>,
}

pub async fn commands(State(s): State<S>, Query(q): Query<CommandsQuery>) -> Result<Json<Vec<serde_json::Value>>, StatusCode> {
    let mid = q.machine_id.as_deref().unwrap_or("");
    let machine_id = Uuid::parse_str(mid).map_err(|_| StatusCode::BAD_REQUEST)?;

    let creating: Vec<Instance> = sqlx::query_as(
        "SELECT * FROM instances WHERE machine_id = $1 AND status = 'creating'")
        .bind(machine_id).fetch_all(&s.pool).await.unwrap_or_default();

    let stopping: Vec<Instance> = sqlx::query_as(
        "SELECT * FROM instances WHERE machine_id = $1 AND status = 'stopping'")
        .bind(machine_id).fetch_all(&s.pool).await.unwrap_or_default();

    let destroying: Vec<Instance> = sqlx::query_as(
        "SELECT * FROM instances WHERE machine_id = $1 AND status = 'destroying'")
        .bind(machine_id).fetch_all(&s.pool).await.unwrap_or_default();

    let mut cmds: Vec<serde_json::Value> = Vec::new();

    for inst in &creating {
        cmds.push(serde_json::json!({
            "id": inst.id.to_string(),
            "type": "CREATE_CONTAINER",
            "instance_id": inst.id.to_string(),
            "payload": {
                "docker_image": inst.docker_image,
                "docker_env": inst.docker_env,
                "docker_cmd": inst.docker_cmd,
                "ssh_public_key": inst.ssh_public_key,
            }
        }));
    }
    for inst in &stopping {
        cmds.push(serde_json::json!({"id": inst.id.to_string(), "type": "STOP_CONTAINER", "instance_id": inst.id.to_string()}));
    }
    for inst in &destroying {
        cmds.push(serde_json::json!({"id": inst.id.to_string(), "type": "DESTROY_CONTAINER", "instance_id": inst.id.to_string()}));
    }

    Ok(Json(cmds))
}

#[derive(Deserialize)]
pub struct CommandAck {
    success: bool,
    result: Option<serde_json::Value>,
    error: Option<String>,
}

pub async fn command_ack(State(s): State<S>, Path(id): Path<Uuid>, Json(ack): Json<CommandAck>) -> StatusCode {
    if ack.success {
        let ssh_port = ack.result.as_ref()
            .and_then(|r| r.get("ssh_port"))
            .and_then(|v| v.as_i64())
            .map(|v| v as i32);

        sqlx::query(
            "UPDATE instances SET status = 'running', started_at = now(), last_billed_at = now(),
             ssh_port = COALESCE($2, ssh_port) WHERE id = $1")
            .bind(id).bind(ssh_port)
            .execute(&s.pool).await.ok();
    } else {
        let err = ack.error.unwrap_or_else(|| "unknown".into());
        sqlx::query("UPDATE instances SET status = 'error' WHERE id = $1")
            .bind(id).execute(&s.pool).await.ok();
        tracing::error!("Command {} failed: {}", id, err);
    }
    StatusCode::OK
}
