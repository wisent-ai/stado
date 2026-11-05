import Foundation

/// `stado inference plan` and `apply` from the Inference screen: a plan is
/// made on the selected API host exactly as the CLI makes it, the endpoint's
/// network chosen by the same `--visibility` the CLI takes, and the plan id it
/// prints applied as one write the API asks to confirm.
enum NativeInferenceOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "inference-plan", title: "Plan an inference deployment on a registered GPU host", path: ["inference", "plan"], hostPlacement: .none, fields: [
            .init(id: "name", label: "Deployment name", required: true),
            .init(id: "host", label: "Registered GPU host", option: "--host", required: true),
            .init(id: "image", label: "Digest-pinned vLLM image (repository@sha256:digest)", option: "--image", required: true),
            .init(id: "model", label: "Hugging Face model repository", option: "--model", required: true),
            .init(id: "revision", label: "Immutable model commit SHA", option: "--revision", required: true),
            .init(id: "port", label: "Endpoint port", option: "--port", required: true),
            .init(id: "visibility", label: "Network the endpoint binds on: lan needs no outside provider, tailscale binds the tailnet address", option: "--visibility", required: true, choices: ["lan", "tailscale"], initial: "lan"),
            .init(id: "gpu", label: "GPU board UUID (GPU-…, as blockers prints it; blank takes the board with the most free memory)", option: "--gpu"),
            .init(id: "max-model-len", label: "Longest context served", option: "--max-model-len", required: true),
            .init(id: "gpu-mode", label: "GPU mode", option: "--gpu-mode", required: true, choices: ["exclusive", "yieldable"], initial: "exclusive"),
            .init(id: "kv-cache-memory-gb", label: "Fixed KV-cache GiB (blank uses the image policy)", option: "--kv-cache-memory-gb"),
            .init(id: "cache-dir", label: "Persistent model cache directory (blank uses the default)", option: "--cache-dir"),
            .init(id: "kind", label: "What it generates: text runs vLLM; image, video and speech need an engine file", option: "--kind", choices: ["text", "image", "video", "speech"], initial: "text"),
            .init(id: "engine-file", label: "Engine declaration JSON for image, video or speech (blank for text)", option: "--engine-file"),
        ]),
        .init(id: "inference-apply", title: "Apply a saved inference plan", path: ["inference", "apply"], hostPlacement: .none, fields: [
            .init(id: "plan-id", label: "Plan id the plan printed", required: true),
        ]),
        .init(id: "inference-route-audit", title: "Routes that pay an outside provider for a model a running deployment serves",
              path: ["inference", "route", "audit"], hostPlacement: .none, mutates: false, jsonOutput: false),
    ]
}
