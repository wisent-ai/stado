use serde_json::Value;

use super::schema::{set_route, validate};

fn document() -> Value {
    serde_json::from_str(
        "{\"schema_version\":2,\"targets\":[{\"name\":\"gateway\",\"kind\":\"local\",\"hostnames\":[\"gateway.local\"]},{\"name\":\"gpu-host-1\",\"kind\":\"local\",\"hostnames\":[\"gpu-host-1.local\"],\"gpu_type\":\"nvidia\",\"vram_gb\":95}],\"inference\":{\"gateway_target\":\"gateway\",\"deployments\":[{\"name\":\"example-serving\",\"target\":\"gpu-host-1\",\"desired_state\":\"running\",\"engine\":{\"name\":\"vllm\",\"image\":\"vllm/vllm-openai@sha256:770fe65b2c73ee74a5c42165cf3433de4048cc2cd9c57a937ca4e35aba5aa87b\"},\"model\":{\"repository\":\"example-org/example-24b-instruct\",\"revision\":\"0123456789abcdef0123456789abcdef01234567\"},\"resources\":{\"gpu_mode\":\"exclusive\",\"gpus\":1,\"max_model_len\":32768},\"endpoint\":{\"host\":\"100.100.1.2\",\"visibility\":\"tailscale\",\"port\":8001,\"protocol\":\"openai-chat\"},\"credential_item\":\"provider:local-openai\"}],\"routes\":{\"example/chat\":\"example/default\"}}}",
    )
    .expect("valid inference fixture")
}

#[test]
fn route_update_requires_expected_value() {
    let document = document();
    validate(&document).expect("valid fixture");
    let next = set_route(
        &document,
        "example/chat",
        "example-serving",
        "example/default",
    )
    .expect("route update");
    assert_eq!(
        next.pointer("/inference/routes/example~1chat")
            .and_then(Value::as_str),
        Some("example-serving")
    );
    assert!(set_route(
        &document,
        "example/chat",
        "example-serving",
        "openai/default",
    )
    .is_err());
}

#[test]
fn deployment_rejects_non_tailscale_endpoint() {
    let document = serde_json::from_str::<Value>(
        &serde_json::to_string(&document())
            .expect("serialize fixture")
            .replace("100.100.1.2", "10.0.0.2"),
    )
    .expect("rewrite fixture");
    let error = validate(&document).expect_err("reject LAN endpoint");
    assert!(error.contains("Tailscale"));
}
