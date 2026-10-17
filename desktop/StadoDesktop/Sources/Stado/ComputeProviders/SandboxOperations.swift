import Foundation

/// The Boat sandbox operations `stado sandbox` has, through the selected
/// endpoint's operator command channel: list and show, create and update,
/// run a command line, read, write and download a file, prompt the coding
/// agent and read the run, interrupt it, read events, allow an SSH key, fork,
/// stop, resume and delete. Each answers with the JSON the CLI prints; the
/// writes run behind the review dialog.
enum NativeSandboxOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "sandbox-list", title: "List every Boat sandbox the key reaches",
            path: ["sandbox", "list"], hostPlacement: .none, mutates: false, jsonOutput: false),
        .init(id: "sandbox-show", title: "Show one sandbox",
            path: ["sandbox", "show"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
            ], mutates: false, jsonOutput: false),
        .init(id: "sandbox-create", title: "Create a sandbox",
            path: ["sandbox", "create"], hostPlacement: .none, fields: [
                .init(id: "ttl", label: "Lifetime in seconds (blank: boat.ttl_seconds)", option: "--ttl-seconds"),
                .init(id: "no-env", label: "Start without the account's environment variables", option: "--no-env", flag: true),
            ], jsonOutput: false),
        .init(id: "sandbox-exec", title: "Run a command line in a sandbox",
            path: ["sandbox", "exec"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "cwd", label: "Directory in the sandbox (blank: the request names none)", option: "--cwd"),
                .init(id: "command", label: "Command line, run by the sandbox's shell", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-read", title: "Read a file from a sandbox",
            path: ["sandbox", "read"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "path", label: "Path in the sandbox", required: true),
                .init(id: "encoding", label: "Encoding", option: "--encoding", required: true, choices: ["utf8", "base64"], initial: "utf8"),
            ], mutates: false, jsonOutput: false),
        .init(id: "sandbox-write", title: "Write a file into a sandbox",
            path: ["sandbox", "write"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "path", label: "Path in the sandbox", required: true),
                .init(id: "from", label: "File on the machine running Stado", option: "--from", required: true),
                .init(id: "encoding", label: "Encoding", option: "--encoding", required: true, choices: ["utf8", "base64"], initial: "utf8"),
            ], jsonOutput: false),
        .init(id: "sandbox-fork", title: "Create a new sandbox from one's disk",
            path: ["sandbox", "fork"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "no-env", label: "Start without the account's environment variables", option: "--no-env", flag: true),
            ], jsonOutput: false),
        .init(id: "sandbox-stop", title: "Stop a sandbox, saving its disk",
            path: ["sandbox", "stop"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-resume", title: "Resume a stopped sandbox",
            path: ["sandbox", "resume"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "no-env", label: "Start without the account's environment variables", option: "--no-env", flag: true),
            ], jsonOutput: false),
        .init(id: "sandbox-delete", title: "Delete a sandbox and its data for good",
            path: ["sandbox", "delete"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-update", title: "Rename a sandbox or change when it stops by itself",
            path: ["sandbox", "update"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "name", label: "New name (blank: unchanged)", option: "--name"),
                .init(id: "ttl", label: "Stop by itself after seconds (blank: unchanged)", option: "--ttl-seconds"),
                .init(id: "no-auto-stop", label: "Never stop by itself", option: "--no-auto-stop", flag: true),
            ], jsonOutput: false),
        .init(id: "sandbox-prompt", title: "Hand the sandbox's coding agent a prompt",
            path: ["sandbox", "prompt"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "prompt", label: "Prompt", required: true),
                .init(id: "provider", label: "Agent provider, as Boat names it", option: "--provider", required: true),
                .init(id: "model", label: "Model (blank: the request names none)", option: "--model"),
                .init(id: "effort", label: "Reasoning effort (blank: the request names none)", option: "--reasoning-effort"),
            ], jsonOutput: false),
        .init(id: "sandbox-prompt-status", title: "Read a prompt run's status",
            path: ["sandbox", "prompt-status"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "prompt-id", label: "Prompt id", required: true),
            ], mutates: false, jsonOutput: false),
        .init(id: "sandbox-interrupt", title: "Interrupt every conversation in a sandbox",
            path: ["sandbox", "interrupt"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-events", title: "Read one page of a sandbox's events",
            path: ["sandbox", "events"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "sort", label: "Order (asc: oldest first, desc: newest first)", option: "--sort", required: true),
                .init(id: "cursor", label: "nextCursor of the previous page (blank: first page)", option: "--cursor"),
                .init(id: "type", label: "Event type (blank: every type)", option: "--type"),
            ], mutates: false, jsonOutput: false),
        .init(id: "sandbox-ssh-key", title: "Allow an OpenSSH public key into a sandbox",
            path: ["sandbox", "ssh-key"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "key-file", label: "Public key file on the machine running Stado", option: "--public-key-file", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-download", title: "Download a file or folder from a sandbox",
            path: ["sandbox", "download"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "path", label: "Path in the sandbox", required: true),
                .init(id: "to", label: "New file on the machine running Stado", option: "--to", required: true),
            ], jsonOutput: false),
    ]
}
