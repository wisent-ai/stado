import Foundation

/// Fleet-owned sandboxes use the same authenticated native operation channel as providers.
/// Every operation but inventory takes the host as optional: empty, `create` places the
/// sandbox on the host with the most room, `list` reads every host and the others find the
/// host that keeps the named sandbox.
enum NativeFleetSandboxOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "fleet-sandbox-inventory", title: "Inspect sandbox runtime prerequisites",
            path: ["sandbox", "fleet", "inventory"], hostPlacement: .none, fields: [
                .init(id: "host", label: "Registered Linux host", option: "--host", required: true),
            ], mutates: false, jsonOutput: false),
        .init(id: "fleet-sandbox-list", title: "List fleet sandboxes on a host or every host",
            path: ["sandbox", "fleet", "list"], hostPlacement: .none, fields: [
                .init(id: "host", label: "Registered Linux host (empty: Stado chooses it)", option: "--host"),
            ], mutates: false, jsonOutput: false),
        .init(id: "fleet-sandbox-create", title: "Create an isolated fleet sandbox",
            path: ["sandbox", "fleet", "create"], hostPlacement: .none, fields: [
                .init(id: "name", label: "Sandbox name", required: true),
                .init(id: "host", label: "Registered Linux host (empty: Stado chooses it)", option: "--host"),
                .init(id: "image", label: "Immutable image (IMAGE@sha256:DIGEST)", option: "--image", required: true),
                .init(id: "cpus", label: "CPU cores", option: "--cpus", required: true),
                .init(id: "memory", label: "RAM ceiling in bytes (no swap)", option: "--memory-bytes", required: true),
                .init(id: "pids", label: "Process ceiling", option: "--pids-limit", required: true),
                .init(id: "stopAfter", label: "Stop after this many seconds of each start (blank: runs until stopped)", option: "--stop-after-seconds"),
                .init(id: "egress", label: "Host name it may reach over HTTP(S) through its own proxy (blank: no network)", option: "--egress"),
                .init(id: "egressProxyImage", label: "Immutable tinyproxy image for that proxy, IMAGE@sha256:DIGEST (with a host)", option: "--egress-proxy-image"),
                .init(id: "gpus", label: "NVIDIA GPUs through gVisor nvproxy (blank: none)", option: "--gpus"),
            ], jsonOutput: false),
        .init(id: "fleet-sandbox-update", title: "Change enforced sandbox resource limits",
            path: ["sandbox", "fleet", "update"], hostPlacement: .none, fields: [
                .init(id: "name", label: "Sandbox name", required: true),
                .init(id: "host", label: "Registered Linux host (empty: Stado chooses it)", option: "--host"),
                .init(id: "cpus", label: "CPU cores", option: "--cpus", required: true),
                .init(id: "memory", label: "RAM ceiling in bytes (no swap)", option: "--memory-bytes", required: true),
                .init(id: "pids", label: "Process ceiling", option: "--pids-limit", required: true),
            ], jsonOutput: false),
        .init(id: "fleet-sandbox-show", title: "Read sandbox state and enforced limits",
            path: ["sandbox", "fleet", "show"], hostPlacement: .none, fields: [
                .init(id: "name", label: "Sandbox name", required: true),
                .init(id: "host", label: "Registered Linux host (empty: Stado chooses it)", option: "--host"),
            ], mutates: false, jsonOutput: false),
        .init(id: "fleet-sandbox-usage", title: "Read what a running sandbox has used since its last start",
            path: ["sandbox", "fleet", "usage"], hostPlacement: .none, fields: [
                .init(id: "name", label: "Sandbox name", required: true),
                .init(id: "host", label: "Registered Linux host (empty: Stado chooses it)", option: "--host"),
            ], mutates: false, jsonOutput: false),
        .init(id: "fleet-sandbox-exec", title: "Execute inside a fleet sandbox",
            path: ["sandbox", "fleet", "exec"], hostPlacement: .none, fields: [
                .init(id: "name", label: "Sandbox name", required: true),
                .init(id: "host", label: "Registered Linux host (empty: Stado chooses it)", option: "--host"),
                .init(id: "command", label: "Command inside the sandbox", option: "--command", required: true),
                .init(id: "detach", label: "Continue in background and retain output", option: "--detach", flag: true),
                .init(id: "credential", label: "Credential field for the command, NAME=ITEM#FIELD (blank: none)", option: "--credential"),
                .init(id: "route", label: "Skarbiec route to read it under a consumer's grant (blank: the selected store)", option: "--route"),
                .init(id: "consumer", label: "Consumer granted the field (with a route)", option: "--consumer"),
                .init(id: "grantFile", label: "File holding that consumer's grant (with a route)", option: "--grant-file"),
            ], jsonOutput: false),
        .init(id: "fleet-sandbox-logs", title: "Read a background execution's output and status",
            path: ["sandbox", "fleet", "logs"], hostPlacement: .none, fields: [
                .init(id: "name", label: "Sandbox name", required: true),
                .init(id: "host", label: "Registered Linux host (empty: Stado chooses it)", option: "--host"),
                .init(id: "execution", label: "Execution UUID returned by detached execution", option: "--execution", required: true),
            ], mutates: false, jsonOutput: false),
        .init(id: "fleet-sandbox-stop", title: "Stop a sandbox, preserving its workspace",
            path: ["sandbox", "fleet", "stop"], hostPlacement: .none, fields: [
                .init(id: "name", label: "Sandbox name", required: true),
                .init(id: "host", label: "Registered Linux host (empty: Stado chooses it)", option: "--host"),
            ], jsonOutput: false),
        .init(id: "fleet-sandbox-resume", title: "Resume a sandbox and its workspace",
            path: ["sandbox", "fleet", "resume"], hostPlacement: .none, fields: [
                .init(id: "name", label: "Sandbox name", required: true),
                .init(id: "host", label: "Registered Linux host (empty: Stado chooses it)", option: "--host"),
            ], jsonOutput: false),
        .init(id: "fleet-sandbox-fork", title: "Fork a stopped sandbox into a new one with a verified copy of its workspace",
            path: ["sandbox", "fleet", "fork"], hostPlacement: .none, fields: [
                .init(id: "name", label: "Stopped source sandbox", required: true),
                .init(id: "host", label: "Registered Linux host (empty: Stado chooses it)", option: "--host"),
                .init(id: "to", label: "Name of the new sandbox", option: "--to", required: true),
            ], jsonOutput: false),
        .init(id: "fleet-sandbox-delete", title: "Delete a sandbox and all workspace data",
            path: ["sandbox", "fleet", "delete"], hostPlacement: .none, fields: [
                .init(id: "name", label: "Sandbox name", required: true),
                .init(id: "host", label: "Registered Linux host (empty: Stado chooses it)", option: "--host"),
                .init(id: "confirm", label: "Repeat the sandbox name to confirm permanent deletion", option: "--confirm", required: true),
            ], jsonOutput: false),
    ]
}
