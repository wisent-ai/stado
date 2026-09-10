import Foundation

enum NativeSpaceOperations {
    private static let reclaimFields: [NativeCapabilityField] = [
        .init(id: "stages", label: "Stages (blank selects every declared stage)", option: "--stage", multiple: true),
    ]
    private static let retirementFields: [NativeCapabilityField] = [
        .init(id: "path", label: "Exact file path on target", required: true),
        .init(id: "product", label: "Owning product", option: "--product", required: true),
    ]
    private static let relocationFields: [NativeCapabilityField] = [
        .init(id: "namespace", label: "Namespace", option: "--namespace", required: true),
        .init(id: "from", label: "Source prefix", option: "--from-prefix", required: true),
        .init(id: "to", label: "Destination prefix (optional)", option: "--to-prefix"),
        .init(id: "root", label: "Declared store root (optional)", option: "--store-root"),
        .init(id: "limit", label: "Item limit (optional)", option: "--limit"),
    ]
    static let all: [NativeCapabilityOperation] = [
        .init(id: "preview-reclaim", title: "Preview selected reclamation stages", path: ["space", "reclaim"],
            hostPlacement: .positional, fields: reclaimFields, fixedArguments: ["--dry-run"], mutates: false),
        .init(id: "apply-reclaim", title: "Apply selected reclamation stages", path: ["space", "reclaim"],
            hostPlacement: .positional, fields: reclaimFields + [
                .init(id: "reason", label: "Reason recorded on target", option: "--reason", required: true),
            ], fixedArguments: ["--apply"]),
        .init(id: "remove-file", title: "Remove one managed file", path: ["space", "file", "remove"],
            hostPlacement: .positional, fields: [.init(id: "path", label: "Exact managed file path", required: true)]),
        .init(id: "preview-retirement", title: "Preview guarded file retirement", path: ["space", "file", "retire"],
            hostPlacement: .positional, fields: retirementFields, fixedArguments: ["--dry-run"], mutates: false),
        .init(id: "apply-retirement", title: "Apply a reviewed file retirement", path: ["space", "file", "retire"],
            hostPlacement: .positional, fields: retirementFields + [
                .init(id: "transaction", label: "Transaction from preview receipt", option: "--transaction", required: true),
                .init(id: "digest", label: "SHA-256 from preview receipt", option: "--expected-sha256", required: true),
                .init(id: "size", label: "Byte size from preview receipt", option: "--expected-size", required: true),
                .init(id: "mode", label: "Mode from preview receipt", option: "--expected-mode", required: true),
            ]),
        .init(id: "preview-relocation", title: "Preview object relocation", path: ["space", "relocate"],
            hostPlacement: .positional, fields: relocationFields, fixedArguments: ["--dry-run"], mutates: false),
        .init(id: "apply-relocation", title: "Apply object relocation", path: ["space", "relocate"],
            hostPlacement: .positional, fields: relocationFields, fixedArguments: ["--apply"]),
    ]
}
