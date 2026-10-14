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
    private static let prefixFields: [NativeCapabilityField] = [
        .init(id: "prefix", label: "Prefixes (blank selects all canonical prefixes)", option: "--prefix", multiple: true),
    ]
    private static let concurrencyFields: [NativeCapabilityField] = [
        .init(id: "concurrency", label: "Objects in parallel (blank measures available parallelism)", option: "--concurrency"),
    ]
    private static let endpointFields: [NativeCapabilityField] = [
        .init(id: "from", label: "Source backend", option: "--from", required: true),
        .init(id: "to", label: "Destination backend", option: "--to", required: true),
        .init(id: "from-path", label: "Source root directory (local)", option: "--from-path"),
        .init(id: "to-path", label: "Destination root directory (local)", option: "--to-path"),
        .init(id: "from-bucket", label: "Source bucket (gcs, s3)", option: "--from-bucket"),
        .init(id: "to-bucket", label: "Destination bucket (gcs, s3)", option: "--to-bucket"),
        .init(id: "from-account", label: "Source account (azure)", option: "--from-account"),
        .init(id: "to-account", label: "Destination account (azure)", option: "--to-account"),
        .init(id: "from-container", label: "Source container (azure)", option: "--from-container"),
        .init(id: "to-container", label: "Destination container (azure)", option: "--to-container"),
        .init(id: "from-region", label: "Source region (s3)", option: "--from-region"),
        .init(id: "to-region", label: "Destination region (s3)", option: "--to-region"),
    ]
    static let all: [NativeCapabilityOperation] = [
        .init(id: "list-store-objects", title: "List objects in the selected API's configured store", path: ["storage", "ls"],
            hostPlacement: .none, fields: [
                .init(id: "prefix", label: "Object prefix (blank reads canonical prefix counts)"),
                .init(id: "limit", label: "Maximum objects (blank lists every object)", option: "--limit"),
                .init(id: "size", label: "Read object bodies to report their sizes", option: "--size", flag: true),
            ], mutates: false),
        .init(id: "copy-store-objects", title: "Copy objects between stores", path: ["storage", "copy"],
            hostPlacement: .none, fields: endpointFields + prefixFields + concurrencyFields, jsonOutput: false),
        .init(id: "backup-store-objects", title: "Copy objects to the configured disaster-recovery store", path: ["storage", "backup"],
            hostPlacement: .none, fields: prefixFields + concurrencyFields, jsonOutput: false),
        .init(id: "verify-store-objects", title: "Compare store names, metadata and bytes", path: ["storage", "verify"],
            hostPlacement: .none, fields: endpointFields + prefixFields, mutates: false),
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
        .init(id: "mount-volume", title: "Mount an attached disk durably", path: ["space", "volume", "mount"],
            hostPlacement: .positional, fields: [
                .init(id: "device", label: "Device leaf, such as sdb1", option: "--device", required: true),
                .init(id: "mountPoint", label: "Absolute mount point on the selected host", option: "--mount-point", required: true),
            ]),
        .init(id: "read-work-root", title: "Read where the agent keeps the fleet's work", path: ["space", "work-root"],
            hostPlacement: .positional, fields: [], mutates: false),
        .init(id: "declare-work-root", title: "Declare the work root on a data volume", path: ["space", "work-root"],
            hostPlacement: .positional, fields: [
                .init(id: "path", label: "Absolute directory on the selected mounted data volume", option: "--path", required: true),
            ]),
    ]
}
