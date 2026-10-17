import Foundation

/// The account-wide Boat operations `stado sandbox` has, beside the ones on
/// one sandbox: the environments every new sandbox can start with (their
/// variables, secret files and repositories), and the webhooks Boat posts
/// every sandbox's lifecycle events to. Each answers with the JSON the CLI
/// prints; the writes run behind the review dialog.
enum NativeSandboxAccountOperations {
    static let all: [NativeCapabilityOperation] = [
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
        .init(id: "sandbox-webhook-list", title: "List the webhooks Boat posts lifecycle events to",
            path: ["sandbox", "webhook", "list"], hostPlacement: .none, mutates: false, jsonOutput: false),
        .init(id: "sandbox-webhook-create", title: "Register a webhook and show its signing secret once",
            path: ["sandbox", "webhook", "create"], hostPlacement: .none, fields: [
                .init(id: "url", label: "Public HTTPS endpoint on port 443", required: true),
                .init(id: "event", label: "Events, comma-separated: sandbox.ready, sandbox.error, sandbox.archived, sandbox.hydrated, sandbox.degraded, sandbox.recovered", option: "--event", required: true),
                .init(id: "name", label: "Name (blank: the request names none)", option: "--name"),
            ], jsonOutput: false),
        .init(id: "sandbox-webhook-show", title: "Show one webhook",
            path: ["sandbox", "webhook", "show"], hostPlacement: .none, fields: [
                .init(id: "webhook", label: "Webhook id", required: true),
            ], mutates: false, jsonOutput: false),
        .init(id: "sandbox-webhook-update", title: "Change a webhook's name, URL or events",
            path: ["sandbox", "webhook", "update"], hostPlacement: .none, fields: [
                .init(id: "webhook", label: "Webhook id", required: true),
                .init(id: "url", label: "New endpoint URL (blank: unchanged)", option: "--url"),
                .init(id: "event", label: "Events from now on, comma-separated (blank: unchanged)", option: "--event"),
                .init(id: "name", label: "New name (blank: unchanged)", option: "--name"),
            ], jsonOutput: false),
        .init(id: "sandbox-webhook-delete", title: "Delete a webhook and its queued deliveries",
            path: ["sandbox", "webhook", "delete"], hostPlacement: .none, fields: [
                .init(id: "webhook", label: "Webhook id", required: true),
            ], jsonOutput: false),
        .init(id: "sandbox-webhook-rotate", title: "Replace a webhook's signing secret and show the new one",
            path: ["sandbox", "webhook", "rotate"], hostPlacement: .none, fields: [
                .init(id: "webhook", label: "Webhook id", required: true),
            ], jsonOutput: false),
    ]
}
