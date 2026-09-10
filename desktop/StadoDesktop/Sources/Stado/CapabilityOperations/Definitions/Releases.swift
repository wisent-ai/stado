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
        .init(id: "submit-source", title: "Submit a release source", path: ["release", "submit"], hostPlacement: .none, fields: [
            .init(id: "source", label: "Git repository path on the selected Stado API host", option: "--source", required: true),
            .init(id: "commit", label: "Full Git commit (blank requires a clean HEAD)", option: "--commit"),
            .init(id: "version", label: "Version declared by that source", option: "--version", required: true),
            .init(id: "channel", label: "Release channel", option: "--channel", choices: ["candidate", "stable"], initial: "candidate"),
        ]),
    ]
}
