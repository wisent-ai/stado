import Foundation

enum NativeHostReleaseOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "declare", title: "Declare a desired binary version", path: ["release", "version", "declare"], fields: [
            .init(id: "binary", label: "Binary", option: "--binary", required: true),
            .init(id: "version", label: "Exact version", option: "--version", required: true),
        ]),
        .init(id: "unset", title: "Withdraw a desired version declaration", path: ["release", "version", "unset"], fields: [
            .init(id: "binary", label: "Binary", option: "--binary", required: true),
        ]),
        .init(id: "promote", title: "Verify and promote a published version", path: ["release", "version", "promote"], fields: [
            .init(id: "binary", label: "Binary", option: "--binary", required: true),
            .init(id: "version", label: "Published version", option: "--version", required: true),
        ]),
        .init(id: "apply", title: "Deliver declared host versions", path: ["release", "version", "converge"], fields: [
            .init(id: "binary", label: "Binary (blank selects all declared binaries)", option: "--binary"),
        ]),
        .init(id: "activate", title: "Activate a verified staged release", path: ["release", "staged", "activate"], fields: [
            .init(id: "product", label: "Product", option: "--product", required: true),
            .init(id: "environment", label: "Declared environment file on target", option: "--env-file", required: true),
            .init(id: "port", label: "Port the activated release must answer on", option: "--port", required: true),
        ]),
        .init(id: "provenance", title: "Read installed artifact provenance", path: ["release", "provenance"], mutates: false),
        .init(id: "local-list", title: "Read this machine's retained Stado releases and any that failed here", path: ["release", "local", "list"], hostPlacement: .none, mutates: false),
        .init(id: "local-restore-previous", title: "Reinstall the Stado release that ran before this one", path: ["release", "local", "restore"], hostPlacement: .none, fixedArguments: ["--previous"], jsonOutput: false),
    ]
}

enum NativeReleaseSourceOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "quality-revision", title: "Check committed gates and the built CLI command tree", path: ["quality", "check"], hostPlacement: .none, fields: [
            .init(id: "root", label: "Repository on the selected Stado API host", option: "--root", required: true),
            .init(id: "revision", label: "Exact committed revision to check", option: "--revision", required: true),
            .init(id: "if-declared", label: "Report a committed declaration with no gates instead of refusing", option: "--if-declared", flag: true),
        ], mutates: false),
        // The one-off commands a model reads in a revision's built CLI, as a
        // list rather than a refusal: `remaining` is what is left to fold
        // into a general primitive.
        .init(id: "quality-command-tree", title: "List the one-off commands of a revision's built CLI", path: ["quality", "command-tree"], hostPlacement: .none, fields: [
            .init(id: "root", label: "Repository on the selected Stado API host", option: "--root", required: true),
            .init(id: "revision", label: "Exact committed revision (blank reads the revision an install builds)", option: "--revision"),
        ], fixedArguments: ["--json"], mutates: false),
        .init(id: "storage-archive", title: "Pack a directory as a release archive", path: ["storage", "archive"], hostPlacement: .none, fields: [
            .init(id: "source", label: "Source directory on the selected Stado API host", required: true),
            .init(id: "output", label: "New archive path on that host, outside the source directory", required: true),
        ]),
        .init(id: "catalog-pin-input", title: "Publish an immutable build input", path: ["release", "catalog", "pin-input"], hostPlacement: .none, fields: [
            .init(id: "checkout", label: "Product checkout on the selected Stado API host", required: true),
            .init(id: "name", label: "Input name (private-cargo-sources for Cargo)", option: "--name", required: true),
            .init(id: "source", label: "Source repository on that host", option: "--source", required: true),
            .init(id: "revision", label: "Committed source revision, tag or branch", option: "--revision", required: true),
            .init(id: "paths", label: "Repository paths, one per line (tree archives only)", option: "--path", multiple: true),
            .init(id: "cargo", label: "Export locked private Cargo dependencies", option: "--cargo", flag: true),
            .init(id: "git-bundle", label: "Export Git bundle for web dependencies (not with Cargo or paths)", option: "--git-bundle", flag: true),
            .init(id: "swiftpm", label: "Export the Swift package's committed resolution (not with Cargo, Git bundle or paths)", option: "--swiftpm", flag: true),
        ]),
        .init(id: "destinations-list", title: "Read declared delivery destinations", path: ["release", "destinations", "list"], hostPlacement: .none, mutates: false),
        .init(id: "destinations-show", title: "Read one product's delivery destinations", path: ["release", "destinations", "show"], hostPlacement: .none, fields: [
            .init(id: "product", label: "Product", required: true),
        ], mutates: false),
        .init(id: "destinations-set", title: "Replace a product's declared delivery destinations", path: ["release", "destinations", "set"], hostPlacement: .none, fields: [
            .init(id: "product", label: "Product", required: true),
            .init(id: "targets", label: "Complete set of registry targets", option: "--target", required: true, multiple: true),
        ]),
        .init(id: "destinations-adopt", title: "Adopt the catalog's existing delivery destinations", path: ["release", "destinations", "adopt"], hostPlacement: .none, fields: [
            .init(id: "product", label: "Product", required: true),
        ]),
        .init(id: "destinations-remove", title: "Remove a product's delivery destination declaration", path: ["release", "destinations", "remove"], hostPlacement: .none, fields: [
            .init(id: "product", label: "Product", required: true),
        ]),
        .init(id: "policy-list", title: "Read every product's rollout policy", path: ["release", "policy", "list"], hostPlacement: .none, mutates: false),
        .init(id: "policy-show", title: "Read one product's rollout policy", path: ["release", "policy", "show"], hostPlacement: .none, fields: [
            .init(id: "product", label: "Product", required: true),
        ], mutates: false),
        .init(id: "policy-apply", title: "Apply a reviewed rollout policy (keeps the active release)", path: ["release", "policy", "apply"], hostPlacement: .none, fields: [
            .init(id: "file", label: "Policy JSON {product, policy} on the selected Stado API host", option: "--file", required: true),
        ]),
        .init(id: "policy-remove-target", title: "Stop releasing a product to one host", path: ["release", "policy", "remove-target"], hostPlacement: .none, fields: [
            .init(id: "product", label: "Product", required: true),
            .init(id: "target", label: "Registry target", option: "--target", required: true),
        ]),
        .init(id: "policy-remove", title: "Stop rolling a product out by release control", path: ["release", "policy", "remove"], hostPlacement: .none, fields: [
            .init(id: "product", label: "Product", required: true),
        ]),
        .init(id: "catalog-adopt-plan", title: "Inspect an iOS checkout before adding it to releases", path: ["release", "catalog", "adopt"], hostPlacement: .none, fields: [
            .init(id: "checkout", label: "Git checkout path on the selected Stado API host", required: true),
            .init(id: "product", label: "Product (blank uses checkout folder)", option: "--product"),
            .init(id: "scheme", label: "Xcode scheme (blank uses project name)", option: "--scheme"),
        ], fixedArguments: ["--kind", "ios-xcode"], mutates: false),
        .init(id: "catalog-adopt", title: "Add an iOS checkout to the release catalog", path: ["release", "catalog", "adopt"], hostPlacement: .none, fields: [
            .init(id: "checkout", label: "Git checkout path on the selected Stado API host", required: true),
            .init(id: "product", label: "Product (blank uses checkout folder)", option: "--product"),
            .init(id: "scheme", label: "Xcode scheme (blank uses project name)", option: "--scheme"),
            .init(id: "owner", label: "Authoritative publisher host", option: "--owner", required: true),
            .init(id: "client", label: "Release submit host", option: "--client", required: true),
            .init(id: "targets", label: "Additional release API hosts (one per line)", option: "--target", multiple: true),
            .init(id: "reloads", label: "Publisher cache HOST=SERVICE pairs (one per line)", option: "--reload", multiple: true),
        ], fixedArguments: ["--kind", "ios-xcode", "--apply"]),
        .init(id: "change-submit", title: "Hand pushed work to a later build (starts no build)", path: ["release", "changes", "submit"], hostPlacement: .none, fields: [
            .init(id: "source", label: "Canonical repository on the Stado API host", option: "--source", required: true),
            .init(id: "commit", label: "Full pushed commit", option: "--commit", required: true),
            .init(id: "task", label: "Oko task identity", option: "--task", required: true),
            .init(id: "session", label: "Author session", option: "--session", required: true),
        ]),
        .init(id: "change-list", title: "Read waiting changes and batch test verdicts", path: ["release", "changes", "list"], hostPlacement: .none, fields: [
            .init(id: "task", label: "Task identity (optional; combined with handoff identities)", option: "--task"),
            .init(id: "ids", label: "Exact handoff identities (one per line; blank does not restrict identities)", option: "--id", multiple: true),
        ], mutates: false),
        // Submitting queues the platform builds and returns the run id; the
        // control host's release agent signs, publishes and delivers once the
        // builds end, and the Releases screen follows the run.
        .init(id: "submit-source", title: "Submit a release source (queues the builds; the release agent finishes the run)", path: ["release", "submit"], hostPlacement: .none, fields: [
            .init(id: "source", label: "Git repository path on the selected Stado API host", option: "--source", required: true),
            .init(id: "commit", label: "Full Git commit (blank requires a clean HEAD)", option: "--commit"),
            .init(id: "version", label: "Version declared by that source", option: "--version", required: true),
            .init(id: "channel", label: "Release channel", option: "--channel", choices: ["candidate", "stable"], initial: "candidate"),
        ]),
        // The same reading the CLI performs, and the same release. `--plan`
        // reads the workspace, keeps the delivery queue of every commit that
        // carries a repair recorded in Oko, and submits nothing, which is why
        // it is the first operation an operator reaches for: the version and
        // the commit come from each checkout, never from a field on this
        // screen. `newest` delivers that queue first, then the rest. The plan
        // also names every open repair no checkout carries (`unplaced`), and
        // `--first` takes the delivery order, comma-separated.
        .init(id: "newest-plan", title: "Read what every product would release, the delivery queue of repaired commits and the repairs no checkout carries (submits nothing)", path: ["release", "newest"], hostPlacement: .none, fields: [
            .init(id: "root", label: "Workspace holding the product checkouts (blank reads the checkout's own workspace)", option: "--root"),
            .init(id: "product", label: "One product (blank reads every product)", option: "--product"),
            .init(id: "first", label: "Products delivered first, in order, comma-separated (blank keeps name order)", option: "--first"),
        ], fixedArguments: ["--plan"], mutates: false),
        // The queue's register as it stands, the same `--register` read the
        // fleet report takes: each commit's state and step, the refusing
        // step's error, and when its release was published and recorded.
        .init(id: "newest-register", title: "Read the delivery queue's register: every kept commit, the step that refused it, and when its release was published and recorded", path: ["release", "newest"], hostPlacement: .none, fixedArguments: ["--register"], mutates: false),
        .init(id: "newest", title: "Deliver the queue of repaired commits, then release every product from the commit and version it declares", path: ["release", "newest"], hostPlacement: .none, fields: [
            .init(id: "root", label: "Workspace holding the product checkouts (blank reads the checkout's own workspace)", option: "--root"),
            .init(id: "product", label: "One product (blank releases every product that is due)", option: "--product"),
            .init(id: "first", label: "Products delivered first, in order, comma-separated (blank keeps name order)", option: "--first"),
            .init(id: "channel", label: "Release channel", option: "--channel", choices: ["candidate", "stable"], initial: "candidate"),
            .init(id: "push", label: "After the first products are delivered and installed here, push every checkout and read its CI, then deliver the rest", option: "--push", flag: true),
        ]),
        // The same rewrite `stado release history` makes: the branch moves
        // only when the plan keeps HEAD's tree, and every refusal is the
        // command's own sentence.
        .init(id: "history", title: "Rewrite a checkout's unpushed commits by a plan (pick, reword, fold, drop, split), keeping its tree", path: ["release", "history"], hostPlacement: .none, fields: [
            .init(id: "root", label: "Checkout whose unpushed commits are rewritten", option: "--root", required: true),
            .init(id: "plan", label: "Plan file: one entry per unpushed commit", option: "--plan", required: true),
            .init(id: "notes-ref", label: "Notes ref an unaltered commit keeps its note under (blank carries no note)", option: "--notes-ref"),
        ]),
        // A build is not a release. These queue and read builds and publish
        // nothing; `release-build` is the release that consumes one, and the
        // CLI refuses it while the build is waiting or has failed.
        .init(id: "build-budget", title: "Read daily build allowance and retained user approvals", path: ["queue", "budget"], hostPlacement: .none, mutates: false),
        .init(id: "build-newest-plan", title: "Read what every product would build (queues nothing)", path: ["build", "newest"], hostPlacement: .none, fields: [
            .init(id: "root", label: "Workspace holding the product checkouts (blank reads the checkout's own workspace)", option: "--root"),
            .init(id: "product", label: "One product (blank reads every product)", option: "--product"),
        ], fixedArguments: ["--plan"], mutates: false),
        .init(id: "build-newest", title: "Build every product from the commit it stands on (releases nothing)", path: ["build", "newest"], hostPlacement: .none, fields: [
            .init(id: "root", label: "Workspace holding the product checkouts (blank reads the checkout's own workspace)", option: "--root"),
            .init(id: "product", label: "One product (blank builds every product)", option: "--product"),
        ]),
        // The daily batch: only the products whose handed-off work no build
        // has taken yet (`stado release changes list` state `queued`).
        .init(id: "build-newest-queued", title: "Build only the products with handed-off work no build has taken yet", path: ["build", "newest"], hostPlacement: .none, fields: [
            .init(id: "root", label: "Workspace holding the product checkouts (blank reads the checkout's own workspace)", option: "--root"),
        ], fixedArguments: ["--queued"]),
        .init(id: "build-submit", title: "Build a committed source (refuses required platforms without post-build tests before queueing)", path: ["build", "submit"], hostPlacement: .none, fields: [
            .init(id: "source", label: "Git repository path on the selected Stado API host", option: "--source", required: true),
            .init(id: "commit", label: "Full Git commit (blank requires a clean HEAD)", option: "--commit"),
            .init(id: "version", label: "Version declared by that source", option: "--version", required: true),
        ]),
        .init(id: "build-status", title: "Read a build and what each platform's job did", path: ["build", "status"], hostPlacement: .none, fields: [
            .init(id: "build", label: "Build ID from build submit or build list", required: true),
        ], mutates: false),
        // The same read as text: each platform's queue wait, every finished
        // step with its exit and duration, the step running now and what it
        // is blocked on — the lines `stado build status` prints in a terminal.
        .init(id: "build-progress", title: "Read what each platform's build job is doing now (steps, durations, waits)", path: ["build", "status"], hostPlacement: .none, fields: [
            .init(id: "build", label: "Build ID from build submit or build list", required: true),
        ], mutates: false, jsonOutput: false),
        .init(id: "build-list", title: "List builds", path: ["build", "list"], hostPlacement: .none, fields: [
            .init(id: "product", label: "One product (blank lists every product's builds)", option: "--product"),
            .init(id: "limit", label: "Only the newest N builds (blank lists every build)", option: "--limit"),
        ], mutates: false),
        .init(id: "release-runs", title: "Read release runs and what each platform's job did", path: ["release", "status"], hostPlacement: .none, fields: [
            .init(id: "product", label: "One product (blank reads every product)"),
            .init(id: "run", label: "One run by id or its first characters (optional)", option: "--run"),
            .init(id: "version", label: "Only runs that published this version (optional)", option: "--version"),
            .init(id: "limit", label: "Only the newest N runs (blank lists every run)", option: "--limit"),
        ], mutates: false),
        .init(id: "release-build", title: "Release a build that has passed (refused while it is waiting or failed)", path: ["release", "submit"], hostPlacement: .none, fields: [
            .init(id: "build", label: "Build ID of a passed build", option: "--build", required: true),
            .init(id: "channel", label: "Release channel", option: "--channel", choices: ["candidate", "stable"], initial: "candidate"),
        ]),
        .init(id: "fetch-release", title: "Fetch a verified archive for an accepted source revision (does not install)", path: ["release", "fetch"], hostPlacement: .none, fields: [
            .init(id: "product", label: "Release product", required: true),
            .init(id: "version", label: "Exact published version", required: true),
            .init(id: "platform", label: "Published platform", option: "--platform", required: true),
            .init(id: "source", label: "Accepted full source commit", option: "--source-commit", required: true),
            .init(id: "destination", label: "Absolute archive filename on the selected Stado API host", option: "--destination", required: true),
        ]),
    ]
}
