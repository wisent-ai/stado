import Foundation

/// The Boat sandbox operations `stado sandbox` has, through the selected
/// endpoint's operator command channel: list and show, create, run a command
/// line, read and write a file, fork, stop, resume and delete. Each answers
/// with the JSON the CLI prints; the writes run behind the review dialog.
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
    ]
}
