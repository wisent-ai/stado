import Foundation

/// The Boat sandbox operations `stado sandbox` has, through the selected
/// endpoint's operator command channel: list and show, create (also from a
/// named snapshot) and update, run a command line, read, write and download
/// a file, prompt the coding agent and read the run, interrupt it, read
/// events, allow an SSH key, give a port a public URL, save, list, show and
/// delete named snapshots, fork, stop, resume and delete. Each answers with
/// the JSON the CLI prints; the writes run behind the review dialog.
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
                .init(id: "from", label: "Named snapshot to start from (blank: a fresh disk)", option: "--from"),
                .init(id: "environment", label: "Environment id to start with (blank: the account's default)", option: "--environment"),
            ], jsonOutput: false),
        .init(id: "sandbox-exec", title: "Run a command line in a sandbox",
            path: ["sandbox", "exec"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "cwd", label: "Directory in the sandbox (blank: the request names none)", option: "--cwd"),
                .init(id: "detach", label: "Start in the background and print its process id", option: "--detach", flag: true),
                .init(id: "command", label: "Command line, run by the sandbox's shell", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-process-status", title: "Read a background command's status and log",
            path: ["sandbox", "process", "status"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "process", label: "Process id from exec --detach", required: true),
            ], mutates: false, jsonOutput: false),
        .init(id: "sandbox-process-stop", title: "Stop a background command",
            path: ["sandbox", "process", "stop"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "process", label: "Process id from exec --detach", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-usage", title: "Read what a sandbox has run and cost",
            path: ["sandbox", "usage"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
            ], mutates: false, jsonOutput: false),
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
                .init(id: "force", label: "Stop even though saving the disk fails, losing what was written since the last snapshot", option: "--force", flag: true),
            ], jsonOutput: false),
        .init(id: "sandbox-recover", title: "Power-cycle a sandbox that stopped answering",
            path: ["sandbox", "recover"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-share", title: "Share a sandbox with the organization that pays for it",
            path: ["sandbox", "share"], hostPlacement: .none, fields: [
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
        .init(id: "sandbox-steer", title: "Give the coding agent's running turn an extra instruction",
            path: ["sandbox", "steer"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "message", label: "Instruction", required: true),
                .init(id: "conversation", label: "Conversation id (blank: the most recently active one)", option: "--conversation"),
            ], jsonOutput: false),
        .init(id: "sandbox-desktop", title: "Get the URL that streams a sandbox's desktop",
            path: ["sandbox", "desktop"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
            ], jsonOutput: false),
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
        .init(id: "sandbox-conversations", title: "Read a sandbox's agent conversations",
            path: ["sandbox", "conversations"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
            ], mutates: false, jsonOutput: false),
        .init(id: "sandbox-history", title: "Read one page of the commands that ran in a sandbox",
            path: ["sandbox", "history"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "mode", label: "Kinds of run, comma-separated: sync, stream, detached, process, commands (blank: every kind)", option: "--mode"),
                .init(id: "query", label: "Text the command line contains (blank: every command)", option: "--query"),
                .init(id: "limit", label: "Commands per page (blank: Boat's page size)", option: "--limit"),
                .init(id: "cursor", label: "nextCursor of the previous page (blank: first page)", option: "--cursor"),
            ], mutates: false, jsonOutput: false),
        .init(id: "sandbox-command", title: "Read one command of a sandbox's history",
            path: ["sandbox", "command"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "command", label: "Command id", required: true),
            ], mutates: false, jsonOutput: false),
        .init(id: "sandbox-logs", title: "Read a command's stored output",
            path: ["sandbox", "logs"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "command", label: "Command id", required: true),
                .init(id: "stream", label: "Output", option: "--stream", required: true, choices: ["stdout", "stderr"], initial: "stdout"),
                .init(id: "encoding", label: "Encoding", option: "--encoding", required: true, choices: ["utf8", "base64"], initial: "utf8"),
                .init(id: "offset", label: "nextOffset of the previous read (blank: from the start)", option: "--offset"),
            ], mutates: false, jsonOutput: false),
        .init(id: "sandbox-processes", title: "Read the processes of a sandbox",
            path: ["sandbox", "processes"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
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
        .init(id: "sandbox-host", title: "Give a port in a sandbox a public HTTPS URL",
            path: ["sandbox", "host"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "port", label: "Port the service listens on", required: true),
                .init(id: "public", label: "Anyone can open it, without its token", option: "--public", flag: true),
                .init(id: "localhost", label: "The service listens on the loopback address only", option: "--localhost", flag: true),
            ], jsonOutput: false),
        .init(id: "sandbox-snapshot-save", title: "Save a sandbox's disk as a named snapshot",
            path: ["sandbox", "snapshot", "save"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
                .init(id: "name", label: "Snapshot name (an existing one is replaced)", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-snapshot-list", title: "List the named snapshots",
            path: ["sandbox", "snapshot", "list"], hostPlacement: .none, mutates: false, jsonOutput: false),
        .init(id: "sandbox-snapshot-show", title: "Show one named snapshot",
            path: ["sandbox", "snapshot", "show"], hostPlacement: .none, fields: [
                .init(id: "name", label: "Snapshot name", required: true),
            ], mutates: false, jsonOutput: false),
        .init(id: "sandbox-snapshot-delete", title: "Delete a named snapshot for good",
            path: ["sandbox", "snapshot", "delete"], hostPlacement: .none, fields: [
                .init(id: "name", label: "Snapshot name", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-snapshot-history", title: "List every snapshot of a sandbox",
            path: ["sandbox", "snapshot", "history"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
            ], mutates: false, jsonOutput: false),
        .init(id: "sandbox-snapshot-latest", title: "Show the snapshot resume and fork start from",
            path: ["sandbox", "snapshot", "latest"], hostPlacement: .none, fields: [
                .init(id: "id", label: "Sandbox id", required: true),
            ], mutates: false, jsonOutput: false),
        .init(id: "sandbox-snapshot-tree", title: "List the files a snapshot holds",
            path: ["sandbox", "snapshot", "tree"], hostPlacement: .none, fields: [
                .init(id: "snapshot", label: "Snapshot id", required: true),
            ], mutates: false, jsonOutput: false),
        .init(id: "sandbox-snapshot-get", title: "Take a file or folder out of a snapshot",
            path: ["sandbox", "snapshot", "get"], hostPlacement: .none, fields: [
                .init(id: "snapshot", label: "Snapshot id", required: true),
                .init(id: "path", label: "Path in the snapshot", required: true),
                .init(id: "to", label: "New file on the machine running Stado", option: "--to", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-environment-list", title: "List the environments",
            path: ["sandbox", "environment", "list"], hostPlacement: .none, mutates: false, jsonOutput: false),
        .init(id: "sandbox-environment-create", title: "Create an empty environment",
            path: ["sandbox", "environment", "create"], hostPlacement: .none, fields: [
                .init(id: "name", label: "Environment name", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-environment-delete", title: "Delete an environment",
            path: ["sandbox", "environment", "delete"], hostPlacement: .none, fields: [
                .init(id: "environment", label: "Environment id", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-environment-set-var", title: "Set an environment variable",
            path: ["sandbox", "environment", "set-var"], hostPlacement: .none, fields: [
                .init(id: "environment", label: "Environment id", required: true),
                .init(id: "key", label: "Variable name", required: true),
            ], payload: .standardInput(label: "The variable's value, one line", initial: ""), jsonOutput: false),
        .init(id: "sandbox-environment-rm-var", title: "Remove an environment variable",
            path: ["sandbox", "environment", "rm-var"], hostPlacement: .none, fields: [
                .init(id: "environment", label: "Environment id", required: true),
                .init(id: "key", label: "Variable name", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-environment-set-file", title: "Set an environment secret file",
            path: ["sandbox", "environment", "set-file"], hostPlacement: .none, fields: [
                .init(id: "environment", label: "Environment id", required: true),
                .init(id: "path", label: "Path in every new sandbox", required: true),
                .init(id: "from", label: "File on the machine running Stado", option: "--from", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-environment-rm-file", title: "Remove an environment secret file",
            path: ["sandbox", "environment", "rm-file"], hostPlacement: .none, fields: [
                .init(id: "environment", label: "Environment id", required: true),
                .init(id: "path", label: "Path in every new sandbox", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-environment-add-repo", title: "Clone a GitHub repository into every new sandbox",
            path: ["sandbox", "environment", "add-repo"], hostPlacement: .none, fields: [
                .init(id: "environment", label: "Environment id", required: true),
                .init(id: "repository", label: "Repository id from Boat's repository list", required: true),
                .init(id: "branch", label: "Branch (blank: the request names none)", option: "--base-branch"),
            ], jsonOutput: false),
    ]
}
