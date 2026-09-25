import Foundation

/// The `stado product` operations the Products screen's per-surface buttons do
/// not cover: the catalog authority, provisioning, reconciliation, the update
/// agents, the canonical-source compilers and the documentation checks. Each
/// runs on the selected Stado API host exactly as the CLI does; the CLI's own
/// value checks refuse a surface, status or operation it does not know, and
/// the API asks for confirmation of every operation that writes.
enum NativeProductOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "registry-add", title: "Register a product in the catalog", path: ["product", "registry", "add"], hostPlacement: .none, fields: [
            .init(id: "id", label: "New product identifier", option: "--id", required: true),
            .init(id: "name", label: "Display name", option: "--name", required: true),
            .init(id: "owner-repository", label: "Canonical GitHub owner/repository", option: "--owner-repository", required: true),
            .init(id: "description", label: "What the product does", option: "--description", required: true),
            .init(id: "visibility", label: "Visibility", option: "--visibility", required: true, choices: ["private", "public"], initial: "private"),
            .init(id: "family", label: "Family", option: "--family", required: true, choices: ["wisent", "standalone"], initial: "wisent"),
            .init(id: "status", label: "Lifecycle status: preview, active or retired", option: "--status", required: true, initial: "preview"),
            .init(id: "evidence", label: "Canonical source references, one per line", option: "--evidence", multiple: true),
            .init(id: "surface", label: "Surfaces KIND=OWNER/REPOSITORY, one per line", option: "--surface", multiple: true),
            .init(id: "installation", label: "Installation declarations, one per line", option: "--installation", multiple: true),
            .init(id: "integration", label: "Integrations, one per line", option: "--integration", multiple: true),
            .init(id: "approved-by", label: "Operator who approved this product", option: "--approved-by"),
            .init(id: "approval-note", label: "Approval provenance", option: "--approval-note"),
        ]),
        .init(id: "registry-set", title: "Update a catalog product", path: ["product", "registry", "set"], hostPlacement: .none, fields: [
            .init(id: "product", label: "Product", required: true),
            .init(id: "name", label: "Display name (blank keeps it)", option: "--name"),
            .init(id: "description", label: "Description (blank keeps it)", option: "--description"),
            .init(id: "status", label: "Lifecycle status (blank keeps it)", option: "--status"),
            .init(id: "add-surface", label: "Surfaces to add or replace, one per line", option: "--add-surface", multiple: true),
            .init(id: "remove-surface", label: "Surfaces to remove, one per line", option: "--remove-surface", multiple: true),
            .init(id: "add-installation", label: "Installations to add or replace, one per line", option: "--add-installation", multiple: true),
            .init(id: "remove-installation", label: "Installation surfaces to remove, one per line", option: "--remove-installation", multiple: true),
            .init(id: "add-integration", label: "Integrations to add or replace, one per line", option: "--add-integration", multiple: true),
            .init(id: "remove-integration", label: "Integration products to remove, one per line", option: "--remove-integration", multiple: true),
        ]),
        .init(id: "registry-rm", title: "Remove a product from the catalog", path: ["product", "registry", "rm"], hostPlacement: .none, fields: [
            .init(id: "product", label: "Product", required: true),
        ], fixedArguments: ["--yes"]),
        .init(id: "create", title: "Provision a preview product and its private repositories", path: ["product", "create"], hostPlacement: .none, fields: [
            .init(id: "allow-create", label: "Authorize private repository creation", option: "--allow-create", flag: true),
        ], payload: .file(option: "--request", label: "Creation request JSON", initial: "{\n}\n")),
        .init(id: "create-status", title: "Read a provisioning request's durable result", path: ["product", "create"], hostPlacement: .none, fields: [
            .init(id: "request", label: "Request ID", option: "--status", required: true),
        ], mutates: false),
        .init(id: "create-resume", title: "Resume a provisioning request", path: ["product", "create"], hostPlacement: .none, fields: [
            .init(id: "request", label: "Request ID", option: "--resume", required: true),
            .init(id: "allow-create", label: "Authorize private repository creation", option: "--allow-create", flag: true),
        ]),
        .init(id: "sync-plan", title: "Show what reconciling installed surfaces would do (changes nothing)", path: ["product", "sync"], hostPlacement: .none, fields: [
            .init(id: "surface", label: "Surface: cli, desktop or service", option: "--surface", required: true, initial: "cli"),
            .init(id: "host", label: "Stado host for service surfaces", option: "--host"),
        ], fixedArguments: ["--dry-run"], mutates: false),
        .init(id: "sync", title: "Reconcile installed surfaces against canonical origin/main", path: ["product", "sync"], hostPlacement: .none, fields: [
            .init(id: "surface", label: "Surface: cli, desktop or service", option: "--surface", required: true, initial: "cli"),
            .init(id: "host", label: "Stado host for service surfaces", option: "--host"),
            .init(id: "fetch", label: "Fetch canonical origins first", option: "--fetch", flag: true),
        ]),
        .init(id: "schedule", title: "Read the product update agents", path: ["product", "schedule"], hostPlacement: .none, mutates: false),
        .init(id: "schedule-install", title: "Install or update the product update agents", path: ["product", "schedule"], hostPlacement: .none, fixedArguments: ["--install"]),
        .init(id: "schedule-remove", title: "Remove the product update agents", path: ["product", "schedule"], hostPlacement: .none, fixedArguments: ["--remove"]),
        .init(id: "paths", title: "Inspect executable ownership and PATH collisions", path: ["product", "paths"], hostPlacement: .none, mutates: false),
        // `--json` belongs to the operation, before the arguments it forwards
        // to the compiler, so it is part of the path here.
        .init(id: "cargo", title: "Run Cargo against canonical source checkouts", path: ["product", "cargo", "--json"], hostPlacement: .none, fields: [
            .init(id: "manifest", label: "Canonical Cargo.toml", option: "--manifest-path", required: true),
            .init(id: "operation", label: "Operation: build, check, test, run or metadata", required: true, initial: "check"),
            .init(id: "forward", label: "Arguments for Cargo, one per line", multiple: true),
        ], jsonOutput: false),
        .init(id: "swift", title: "Build or index canonical Swift sources", path: ["product", "swift", "--json"], hostPlacement: .none, fields: [
            .init(id: "package", label: "Canonical package directory", option: "--package-path", required: true),
            .init(id: "editor-workspace", label: "Separate editor workspace for index declarations", option: "--editor-workspace"),
            .init(id: "operation", label: "Operation: build, test, run or index", required: true, initial: "build"),
            .init(id: "forward", label: "Arguments for SwiftPM, one per line", multiple: true),
        ], jsonOutput: false),
        .init(id: "documentation-pages", title: "Verify the product websites' command pages", path: ["product", "documentation", "cli-pages"], hostPlacement: .none, fields: [
            .init(id: "origin", label: "Documentation origins, one per line (blank checks every one)", option: "--origin", multiple: true),
        ], mutates: false, jsonOutput: false),
        .init(id: "documentation-markdown", title: "Find repository Markdown beyond README.md", path: ["product", "documentation", "markdown-policy"], hostPlacement: .none, fields: [
            .init(id: "org", label: "GitHub organization (blank reads wisent-ai)", option: "--org"),
            .init(id: "archived", label: "Include archived repositories", option: "--include-archived", flag: true),
        ], mutates: false, jsonOutput: false),
    ]
}
