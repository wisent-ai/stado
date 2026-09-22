import Foundation

enum NativeHostReleaseOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "declare", title: "Declare a desired binary version", path: ["release", "declare-version"], fields: [
            .init(id: "binary", label: "Binary", option: "--binary", required: true),
            .init(id: "version", label: "Exact version", option: "--version", required: true),
        ]),
        .init(id: "unset", title: "Withdraw a desired version declaration", path: ["release", "declare-version"], fields: [
            .init(id: "binary", label: "Binary", option: "--binary", required: true),
        ], fixedArguments: ["--unset"]),
        .init(id: "promote", title: "Verify and promote a published version", path: ["release", "promote-version"], fields: [
            .init(id: "binary", label: "Binary", option: "--binary", required: true),
            .init(id: "version", label: "Published version", option: "--version", required: true),
        ]),
        .init(id: "apply", title: "Deliver declared host versions", path: ["release", "host-state"], fields: [
            .init(id: "binary", label: "Binary (blank selects all declared binaries)", option: "--binary"),
        ], fixedArguments: ["--apply"]),
        .init(id: "verify-platform", title: "Run the declared native release journeys", path: ["release", "verify-platform"], fields: [
            .init(id: "repository", label: "Repository path", option: "--repo", required: true),
            .init(id: "revision", label: "Exact source revision", option: "--ref", required: true),
        ]),
        .init(id: "activate", title: "Activate a verified staged release", path: ["release", "activate-staged"], fields: [
            .init(id: "product", label: "Product (optional)", option: "--product"),
            .init(id: "environment", label: "Declared environment file on target (optional)", option: "--env-file"),
            .init(id: "port", label: "Declared probe port (optional)", option: "--port"),
        ]),
        .init(id: "provenance", title: "Read installed artifact provenance", path: ["release", "provenance"], mutates: false),
    ]
}

enum NativeReleaseSourceOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "change-submit", title: "Hand pushed work to a later build (starts no build)", path: ["release", "changes", "submit"], hostPlacement: .none, fields: [
            .init(id: "source", label: "Canonical repository on the Stado API host", option: "--source", required: true),
            .init(id: "commit", label: "Full pushed commit", option: "--commit", required: true),
            .init(id: "task", label: "Oko task identity", option: "--task", required: true),
            .init(id: "session", label: "Author session", option: "--session", required: true),
        ]),
        .init(id: "change-list", title: "Read waiting changes and batch test verdicts", path: ["release", "changes", "list"], hostPlacement: .none, fields: [
            .init(id: "task", label: "Task identity (blank reads all)", option: "--task"),
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
        // reads the workspace and submits nothing, which is why it is the
        // first operation an operator reaches for: the version and the commit
        // come from each checkout, never from a field on this screen.
        .init(id: "newest-plan", title: "Read what every product would release (submits nothing)", path: ["release", "newest"], hostPlacement: .none, fields: [
            .init(id: "root", label: "Workspace holding the product checkouts (blank reads the checkout's own workspace)", option: "--root"),
            .init(id: "product", label: "One product (blank reads every product)", option: "--product"),
        ], fixedArguments: ["--plan"], mutates: false),
        .init(id: "newest", title: "Release every product from the commit and version it declares", path: ["release", "newest"], hostPlacement: .none, fields: [
            .init(id: "root", label: "Workspace holding the product checkouts (blank reads the checkout's own workspace)", option: "--root"),
            .init(id: "product", label: "One product (blank releases every product that is due)", option: "--product"),
            .init(id: "channel", label: "Release channel", option: "--channel", choices: ["candidate", "stable"], initial: "candidate"),
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
        .init(id: "build-submit", title: "Build a source (queues the platform builds; publishes nothing)", path: ["build", "submit"], hostPlacement: .none, fields: [
            .init(id: "source", label: "Git repository path on the selected Stado API host", option: "--source", required: true),
            .init(id: "commit", label: "Full Git commit (blank requires a clean HEAD)", option: "--commit"),
            .init(id: "version", label: "Version declared by that source", option: "--version", required: true),
        ]),
        .init(id: "build-status", title: "Read a build and what each platform's job did", path: ["build", "status"], hostPlacement: .none, fields: [
            .init(id: "build", label: "Build ID from build submit or build list", required: true),
        ], mutates: false),
        .init(id: "build-list", title: "List recent builds", path: ["build", "list"], hostPlacement: .none, fields: [
            .init(id: "product", label: "One product (blank lists every product's builds)", option: "--product"),
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
