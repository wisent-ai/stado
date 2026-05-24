use bollard::Docker;
use bollard::auth::DockerCredentials;
use bollard::container::{Config, CreateContainerOptions, StartContainerOptions, StopContainerOptions, RemoveContainerOptions, ListContainersOptions};
use bollard::image::CreateImageOptions;
use bollard::models::{DeviceRequest, HostConfig, PortBinding};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Fetch a GCE service-account access token from the instance metadata server
/// for authenticating `docker pull` against private Artifact Registry / GCR
/// repos. Returns DockerCredentials in the oauth2accesstoken form bollard
/// passes via X-Registry-Auth, since the daemon itself does NOT read any
/// user's docker config.json.
async fn gcp_registry_credentials(image: &str) -> Option<DockerCredentials> {
    if !(image.contains(".pkg.dev") || image.contains("gcr.io")) {
        return None;
    }
    let resp = reqwest::Client::new()
        .get("http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token")
        .header("Metadata-Flavor", "Google")
        .send().await.ok()?;
    if !resp.status().is_success() { return None; }
    let body: serde_json::Value = resp.json().await.ok()?;
    let token = body.get("access_token")?.as_str()?.to_string();
    let server = image.split('/').next().unwrap_or("").to_string();
    Some(DockerCredentials {
        username: Some("oauth2accesstoken".into()),
        password: Some(token),
        serveraddress: Some(server),
        ..Default::default()
    })
}

#[derive(Debug, Deserialize)]
pub struct AgentCommand {
    pub id: String,
    #[serde(rename = "type")]
    pub cmd_type: String,
    pub instance_id: String,
    pub payload: Option<CreatePayload>,
}

#[derive(Debug, Deserialize)]
pub struct CreatePayload {
    pub docker_image: String,
    pub docker_env: Option<HashMap<String, String>>,
    pub docker_cmd: Option<String>,
    pub ssh_public_key: Option<String>,
}

#[derive(Serialize)]
struct Ack {
    success: bool,
    result: Option<serde_json::Value>,
    error: Option<String>,
}

pub async fn poll_commands(client: &Client, docker: &Docker, server: &str, machine_id: &str, token: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{server}/api/v1/agent/commands");
    let resp = client.get(&url)
        .bearer_auth(token)
        .query(&[("machine_id", machine_id)])
        .send().await?;

    if !resp.status().is_success() { return Ok(()); }

    let commands: Vec<AgentCommand> = resp.json().await.unwrap_or_default();
    for cmd in commands {
        let result = execute(docker, &cmd).await;
        let ack = match &result {
            Ok(v) => Ack { success: true, result: Some(v.clone()), error: None },
            Err(e) => Ack { success: false, result: None, error: Some(e.to_string()) },
        };
        let ack_url = format!("{server}/api/v1/agent/commands/{}/ack", cmd.id);
        client.post(&ack_url).bearer_auth(token).json(&ack).send().await.ok();
    }
    Ok(())
}

async fn execute(docker: &Docker, cmd: &AgentCommand) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    match cmd.cmd_type.as_str() {
        "CREATE_CONTAINER" => create(docker, cmd).await,
        "STOP_CONTAINER" => stop(docker, &cmd.instance_id).await,
        "START_CONTAINER" => start(docker, &cmd.instance_id).await,
        "DESTROY_CONTAINER" => destroy(docker, &cmd.instance_id).await,
        other => Err(format!("unknown: {other}").into()),
    }
}

async fn create(docker: &Docker, cmd: &AgentCommand) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let p = cmd.payload.as_ref().ok_or("missing payload")?;
    // Yield if a Vast.ai rental is active on this host so we do not
    // starve a paid renter mid-lease. Scheduler retries on the next poll.
    if crate::vast::is_vast_renting(docker).await {
        return Err("vast rental active on this host; yielding".into());
    }
    let name = format!("wisent-{}", &cmd.instance_id[..8.min(cmd.instance_id.len())]);

    // Pull. The daemon doesn't read any user's docker config.json, so for
    // private Artifact Registry / GCR images we fetch a GCE service-account
    // access token from the metadata server and pass it via the bollard
    // credentials arg (becomes X-Registry-Auth). Propagate pull errors so
    // the marketplace marks the instance failed instead of silently moving
    // on to create_container, which would then 404.
    let opts = CreateImageOptions { from_image: p.docker_image.as_str(), ..Default::default() };
    let creds = gcp_registry_credentials(&p.docker_image).await;
    let mut stream = docker.create_image(Some(opts), None, creds);
    let mut pull_err: Option<String> = None;
    while let Some(info) = stream.next().await {
        if let Err(e) = info {
            tracing::error!("pull: {e}");
            pull_err = Some(e.to_string());
        }
    }
    if let Some(e) = pull_err { return Err(format!("docker pull failed: {e}").into()); }

    let mut env: Vec<String> = Vec::new();
    if let Some(vars) = &p.docker_env { for (k, v) in vars { env.push(format!("{k}={v}")); } }
    if let Some(key) = &p.ssh_public_key { env.push(format!("SSH_PUBLIC_KEY={key}")); }

    let ssh_port = 30000 + (rand::random::<u16>() % 10000);
    let jupyter_port = ssh_port + 1;

    let mut port_bindings = HashMap::new();
    port_bindings.insert("22/tcp".to_string(), Some(vec![PortBinding { host_ip: Some("0.0.0.0".into()), host_port: Some(ssh_port.to_string()) }]));
    port_bindings.insert("8888/tcp".to_string(), Some(vec![PortBinding { host_ip: Some("0.0.0.0".into()), host_port: Some(jupyter_port.to_string()) }]));

    let host_config = HostConfig {
        port_bindings: Some(port_bindings),
        device_requests: Some(vec![DeviceRequest { count: Some(-1), capabilities: Some(vec![vec!["gpu".into()]]), ..Default::default() }]),
        ..Default::default()
    };

    let config = Config {
        image: Some(p.docker_image.clone()),
        env: Some(env),
        host_config: Some(host_config),
        cmd: p.docker_cmd.as_ref().map(|c| vec!["bash".into(), "-c".into(), c.clone()]),
        labels: Some(HashMap::from([("wisent.instance_id".into(), cmd.instance_id.clone())])),
        ..Default::default()
    };

    let opts = CreateContainerOptions { name: name.as_str(), ..Default::default() };
    let created = docker.create_container(Some(opts), config).await?;
    docker.start_container(&created.id, None::<StartContainerOptions<String>>).await?;

    Ok(serde_json::json!({"container_id": created.id, "ssh_port": ssh_port, "jupyter_port": jupyter_port}))
}

async fn find_container(docker: &Docker, instance_id: &str) -> Option<String> {
    let opts = ListContainersOptions::<String> { all: true, ..Default::default() };
    if let Ok(containers) = docker.list_containers(Some(opts)).await {
        for c in containers {
            if let Some(labels) = &c.labels {
                if labels.get("wisent.instance_id").map(|s| s.as_str()) == Some(instance_id) {
                    return c.id.clone();
                }
            }
        }
    }
    None
}

async fn stop(docker: &Docker, instance_id: &str) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(id) = find_container(docker, instance_id).await {
        docker.stop_container(&id, Some(StopContainerOptions { t: 30 })).await?;
    }
    Ok(serde_json::json!({"stopped": true}))
}

async fn start(docker: &Docker, instance_id: &str) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(id) = find_container(docker, instance_id).await {
        docker.start_container(&id, None::<StartContainerOptions<String>>).await?;
    }
    Ok(serde_json::json!({"started": true}))
}

async fn destroy(docker: &Docker, instance_id: &str) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(id) = find_container(docker, instance_id).await {
        docker.remove_container(&id, Some(RemoveContainerOptions { force: true, ..Default::default() })).await?;
    }
    Ok(serde_json::json!({"destroyed": true}))
}
