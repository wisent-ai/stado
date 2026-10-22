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
            .init(id: "add-dependency", label: "Needs of another product to add or replace, one JSON object per line: {\"product\",\"surface\",\"for_surface\",\"alternative\",\"alternative_source\"} or {…,\"bound_because\"}", option: "--add-dependency", multiple: true),
            .init(id: "remove-dependency", label: "Products this one no longer needs, one per line: every declared need on each is removed", option: "--remove-dependency", multiple: true),
            .init(id: "add-rival", label: "Rivals to add or replace, one JSON object per line: {\"id\",\"name\",\"url\",\"evidence\"}", option: "--add-rival", multiple: true),
            .init(id: "remove-rival", label: "Rival ids to remove, one per line", option: "--remove-rival", multiple: true),
            .init(id: "add-roadmap", label: "Roadmap items to add or replace, one JSON object per line: {\"title\",\"status\",\"outcome\",\"source\"}", option: "--add-roadmap", multiple: true),
            .init(id: "remove-roadmap", label: "Roadmap item titles to remove, one per line", option: "--remove-roadmap", multiple: true),
            .init(id: "benchmark", label: "Benchmark measuring the rivals, as JSON: {\"app\",\"suites\"}", option: "--benchmark"),
            .init(id: "remove-benchmark", label: "Remove the declared benchmark", option: "--remove-benchmark", flag: true),
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
        // The answer carries the lock holder or its post-placement operation,
        // including the recorded step, pid and observed process state.
        .init(id: "status", title: "Read one installation and its active operation", path: ["product", "status"], hostPlacement: .none, fields: [
            .init(id: "product", label: "Product", required: true),
            .init(id: "surface", label: "Surface: cli, desktop or service", option: "--surface", required: true, initial: "cli"),
            .init(id: "host", label: "Stado host for a service surface", option: "--host"),
        ], mutates: false),
        .init(id: "paths", title: "Inspect executable ownership and PATH collisions", path: ["product", "paths"], hostPlacement: .none, mutates: false),
        // Ends the running application through LaunchServices, waits on its
        // process in the wait register, opens the bundle and reads it running.
        .init(id: "restart", title: "Restart a local macOS application bundle after its installed bytes change", path: ["product", "restart"], hostPlacement: .none, fields: [
            .init(id: "bundle", label: "Application bundle (APP.app) on this Mac", required: true),
            .init(id: "if-running", label: "Leave a stopped application stopped", option: "--if-running", flag: true),
        ]),
        // The answer lists, per product, each product its code reaches, the
        // standing the catalog gives it and every line that reaches it; a
        // product not shown running alone ends the operation as refused.
        .init(id: "dependencies", title: "Read which products each product's code needs, and refuse one with no way to run without them", path: ["product", "dependencies"], hostPlacement: .none, fields: [
            .init(id: "product", label: "Product (blank reads every catalog product)"),
        ], mutates: false),
        // The name register: every active repository of the organization with
        // the product the catalog claims it for, what it is (its GitHub
        // description, else what the catalog says: the product's description
        // for its owner repository, the surface kind and product for any
        // other) and the `from` column naming which source the row read. The
        // register is Markdown, which `--names` writes in place of `--json`;
        // a blank organization leaves `--names` to the CLI's own default.
        .init(id: "catalog-names", title: "Read the name register: what each repository of the organization is, and where that was read", path: ["product", "catalog", "--names"], hostPlacement: .none, fields: [
            .init(id: "org", label: "GitHub organization (blank reads the CLI's default)"),
        ], mutates: false, jsonOutput: false),
        .init(id: "catalog-names-write", title: "Write the name register to a file (~/NAMES.md)", path: ["product", "catalog", "--names"], hostPlacement: .none, fields: [
            .init(id: "org", label: "GitHub organization (blank reads the CLI's default)"),
            .init(id: "output", label: "Register file to write", option: "--output", required: true),
        ], jsonOutput: false),
        // Refused, naming the file, when it differs from the register the
        // organization and the catalog give now.
        .init(id: "catalog-names-check", title: "Check that a name register file is the one the organization and the catalog give now", path: ["product", "catalog", "--names"], hostPlacement: .none, fields: [
            .init(id: "org", label: "GitHub organization (blank reads the CLI's default)"),
            .init(id: "check", label: "Register file to check", option: "--check", required: true),
        ], mutates: false, jsonOutput: false),
        // Every committed Cargo.lock of the selected checkouts read against an
        // OSV advisory database: each affected package version, its advisory,
        // the fixed versions and whether Cargo reaches a fix without a
        // manifest change. A finding that stands ends the operation as refused.
        .init(id: "advisories-list", title: "Read the published advisories that affect what each checkout's Cargo.lock resolves", path: ["product", "advisories", "list"], hostPlacement: .none, fields: [
            .init(id: "product", label: "Product (blank reads every checkout in the workspace)"),
            .init(id: "advisories", label: "Advisory ids or aliases, one per line (blank reads every advisory)", option: "--advisory", multiple: true),
            .init(id: "database", label: "OSV database origin (blank reads the public OSV database)", option: "--database"),
        ], mutates: false),
        // Rewrites the lockfiles of the selected checkouts with cargo update
        // --precise, names why each other affected package cannot move, and
        // commits nothing.
        .init(id: "advisories-apply", title: "Lock each advisory's first compatible fixed version in every checkout's Cargo.lock", path: ["product", "advisories", "apply"], hostPlacement: .none, fields: [
            .init(id: "product", label: "Product (blank updates every checkout in the workspace)"),
            .init(id: "advisories", label: "Advisory ids or aliases, one per line (blank updates for every advisory)", option: "--advisory", multiple: true),
            .init(id: "database", label: "OSV database origin (blank reads the public OSV database)", option: "--database"),
        ]),
        // The answer lists each workflow the product's repository declares
        // with its newest run, its conclusion and the failed job and step of
        // a run that did not pass, read from the forge as the CLI reads it,
        // and the delivery routine: its plan, the step it is installing and
        // for how long, the refused step with every refusal and its defects.
        .init(id: "ci-status", title: "Read what the forge's continuous integration and the delivery routine last did", path: ["ci", "status"], hostPlacement: .none, fields: [
            .init(id: "product", label: "Product of the catalog", required: true),
            .init(id: "revision", label: "Full commit (blank reads the newest run of any revision)", option: "--revision"),
        ], mutates: false),
        // Whether the delivery path stands, and what the main check found on
        // each step's main: every commit checked, green or red with every
        // refusal and its Oko defects, and the last green commit, which is
        // what the routine installs. A red main is named under `stalled`.
        .init(id: "ci-delivery", title: "Read the delivery path and the main check of every step: each commit green or red, and the last green one installed", path: ["ci", "delivery"], hostPlacement: .none, mutates: false),
        // The delivery routine checks every new commit of each step's main,
        // installs each step's last green commit, in order, then pushes every
        // checkout when declared; Stado's scheduler runs `ci run`.
        .init(id: "ci-plan-set", title: "Declare the products the delivery routine installs, in order", path: ["ci", "plan", "set"], hostPlacement: .none, fields: [
            .init(id: "purpose", label: "Open Oko defect the installs repair (the operator's rulings are asked for this purpose)", option: "--purpose", required: true),
            .init(id: "steps", label: "Steps, one PRODUCT:SURFACE:CHECKOUT per line, in install order", option: "--step", required: true, multiple: true),
            .init(id: "push", label: "Push main of every checkout once every step is installed", option: "--push", flag: true),
        ]),
        .init(id: "ci-plan-show", title: "Read the delivery routine's declared plan", path: ["ci", "plan", "show"], hostPlacement: .none, mutates: false),
        .init(id: "ci-plan-clear", title: "Remove the delivery routine's plan", path: ["ci", "plan", "clear"], hostPlacement: .none, jsonOutput: false),
        // One pass: the main check over every new commit, then the installs.
        // A refused step ends the operation as refused with every refusal
        // and the Oko defects recorded for it.
        .init(id: "ci-run", title: "Run one pass of the delivery routine: check every new commit of main, install each step's last green commit", path: ["ci", "run"], hostPlacement: .none),
        // The components each product declares under `public_status`, as the
        // status page reads them: state, since when and the last failed
        // check with the answer that failed it. `check` asks every component
        // once from the selected host and publishes the result.
        .init(id: "public-status-show", title: "Read the published state of every component products declare for their public status", path: ["public-status", "show"], hostPlacement: .none, mutates: false),
        .init(id: "public-status-check", title: "Ask every declared component once and publish its state, since when and the last failure", path: ["public-status", "check"], hostPlacement: .none),
        // `--json` belongs to the operation, before the arguments it forwards
        // to the compiler, so it is part of the path here.
        .init(id: "cargo", title: "Run Cargo against canonical checkouts or release inputs", path: ["product", "cargo", "--json"], hostPlacement: .none, fields: [
            .init(id: "manifest", label: "Cargo.toml on the selected Stado API host", option: "--manifest-path", required: true),
            .init(id: "operation", label: "Operation: build, check, test, run, metadata, stage, or a Cargo subcommand the tree's compiling quality gates run (clippy)", required: true, initial: "check"),
            .init(id: "forward", label: "Arguments for Cargo, one per line", multiple: true),
        ], jsonOutput: false),
        // The receipts `cargo` retained: each run's state, operation, pinned
        // sources, evidence directory and, for a refused one, its error.
        .init(id: "cargo-results", title: "Read the Cargo runs retained for a checkout, or for every workspace checkout", path: ["product", "cargo", "--json"], hostPlacement: .none, fields: [
            .init(id: "manifest", label: "Cargo.toml of one checkout on the selected Stado API host (blank when every checkout is read)", option: "--manifest-path"),
            .init(id: "all", label: "Every canonical workspace checkout", option: "--all", flag: true),
        ], fixedArguments: ["results"], mutates: false, jsonOutput: false),
        // The answer names the changelog, the entries moved and every file written.
        .init(id: "changelog", title: "Move CHANGELOG.md's Unreleased entries into changelog/: as a release, or spilled while it is prepared", path: ["product", "changelog"], hostPlacement: .none, fields: [
            .init(id: "root", label: "Repository checkout on the selected Stado API host", option: "--root", required: true),
            .init(id: "line-limit", label: "The repository's file length limit, in lines", option: "--line-limit", required: true),
            .init(id: "version", label: "Release the entries go out in (blank with spill set)", option: "--version"),
            .init(id: "spill", label: "Spill the entries written so far into changelog/unreleased-part-N.md", option: "--spill", flag: true),
        ], jsonOutput: false),
        .init(id: "swift", title: "Build or index canonical Swift sources", path: ["product", "swift", "--json"], hostPlacement: .none, fields: [
            .init(id: "package", label: "Canonical package directory", option: "--package-path", required: true),
            .init(id: "editor-workspace", label: "Separate editor workspace for index declarations", option: "--editor-workspace"),
            .init(id: "operation", label: "Operation: build, test, run or index", required: true, initial: "build"),
            .init(id: "forward", label: "Arguments for SwiftPM, one per line", multiple: true),
        ], jsonOutput: false),
        .init(id: "swift-restore", title: "Restore locked Swift release dependencies", path: ["product", "swift", "--json"], hostPlacement: .none, fields: [
            .init(id: "package", label: "Release source directory with Package.resolved", option: "--package-path", required: true),
            .init(id: "archive", label: "Portable SwiftPM input archive", option: "--archive", required: true),
        ], fixedArguments: ["restore"], jsonOutput: false),
        .init(id: "editor-status", title: "Inspect live editor instances, parent processes and connection age", path: ["product", "editor", "status"], hostPlacement: .none, fields: [
            .init(id: "package", label: "Canonical package directory (blank lists every active workspace)", option: "--package-path"),
        ], mutates: false),
        .init(id: "documentation-pages", title: "Verify the product websites' command pages", path: ["product", "documentation", "cli-pages"], hostPlacement: .none, fields: [
            .init(id: "origin", label: "Documentation origins, one per line (blank checks every one)", option: "--origin", multiple: true),
        ], mutates: false, jsonOutput: false),
        .init(id: "documentation-markdown", title: "Find repository Markdown beyond README.md", path: ["product", "documentation", "markdown-policy"], hostPlacement: .none, fields: [
            .init(id: "org", label: "GitHub organization (blank reads wisent-ai)", option: "--org"),
            .init(id: "archived", label: "Include archived repositories", option: "--include-archived", flag: true),
        ], mutates: false, jsonOutput: false),
    ]
}
