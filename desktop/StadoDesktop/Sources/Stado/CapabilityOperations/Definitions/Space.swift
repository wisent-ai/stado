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
        .init(id: "from-bucket", label: "Source bucket (gcs, s3, s3-compatible, named S3 providers)", option: "--from-bucket"),
        .init(id: "to-bucket", label: "Destination bucket (gcs, s3, s3-compatible, named S3 providers)", option: "--to-bucket"),
        .init(id: "from-account", label: "Source account (azure; cloudflare, oracle, upcloud)", option: "--from-account"),
        .init(id: "to-account", label: "Destination account (azure; cloudflare, oracle, upcloud)", option: "--to-account"),
        .init(id: "from-container", label: "Source container (azure)", option: "--from-container"),
        .init(id: "to-container", label: "Destination container (azure)", option: "--to-container"),
        .init(id: "from-region", label: "Source region (s3, s3-compatible, named S3 providers)", option: "--from-region"),
        .init(id: "to-region", label: "Destination region (s3, s3-compatible, named S3 providers)", option: "--to-region"),
        .init(id: "from-endpoint", label: "Source S3 API endpoint URL (s3-compatible, minio, garage, seaweedfs, ceph, idrive-e2)", option: "--from-endpoint"),
        .init(id: "to-endpoint", label: "Destination S3 API endpoint URL (s3-compatible, minio, garage, seaweedfs, ceph, idrive-e2)", option: "--to-endpoint"),
        .init(id: "from-addressing", label: "Source addressing (s3-compatible)", option: "--from-addressing",
            choices: ["", "path", "virtual-hosted"]),
        .init(id: "to-addressing", label: "Destination addressing (s3-compatible)", option: "--to-addressing",
            choices: ["", "path", "virtual-hosted"]),
        .init(id: "from-credential-role", label: "Source credential role holding the key pair (s3-compatible)",
            option: "--from-credential-role"),
        .init(id: "to-credential-role", label: "Destination credential role holding the key pair (s3-compatible)",
            option: "--to-credential-role"),
    ]
    private static let bucketFields: [NativeCapabilityField] = [
        .init(id: "name", label: "Bucket (namespace)", required: true),
    ]
    private static let bucketGrantFields: [NativeCapabilityField] = [
        .init(id: "grants", label: "Grants, one PREFIX=ACTION,ACTION per line (actions: get, stat, list, put, delete)",
            option: "--grant", required: true, multiple: true),
        .init(id: "reload", label: "Unit to restart afterwards (optional)", option: "--reload-service"),
    ]
    static let all: [NativeCapabilityOperation] = [
        .init(id: "list-store-objects", title: "List objects in the selected API's configured store", path: ["storage", "ls"],
            hostPlacement: .none, fields: [
                .init(id: "prefix", label: "Object prefix (blank reads canonical prefix counts)"),
                .init(id: "limit", label: "Maximum objects (blank lists every object)", option: "--limit"),
                .init(id: "size", label: "Report each object's size, as the store's listing states it", option: "--size", flag: true),
            ], mutates: false),
        .init(id: "copy-store-objects", title: "Copy objects between stores", path: ["storage", "copy"],
            hostPlacement: .none, fields: endpointFields + prefixFields + concurrencyFields, jsonOutput: false),
        .init(id: "backup-store-objects", title: "Copy objects to the configured disaster-recovery store", path: ["storage", "backup"],
            hostPlacement: .none, fields: prefixFields + concurrencyFields, jsonOutput: false),
        .init(id: "verify-store-objects", title: "Compare store names, metadata and bytes", path: ["storage", "verify"],
            hostPlacement: .none, fields: endpointFields + prefixFields, mutates: false),
        .init(id: "object-url", title: "Print an object's URL, or a presigned S3 URL that expires", path: ["storage", "url"],
            hostPlacement: .none, fields: [
                .init(id: "uri", label: "Object (stado://<namespace>/<key>)", required: true),
                .init(id: "presign", label: "Presign for (blank prints the bearer URL)", option: "--presign",
                    choices: ["", "get", "put"]),
                .init(id: "expires", label: "Seconds the presigned URL stays valid (1 to 604800)", option: "--expires"),
            ], mutates: false),
        .init(id: "s3-endpoint", title: "Describe and check a namespace's S3 endpoint", path: ["storage", "s3-endpoint"],
            hostPlacement: .none, fields: [
                .init(id: "namespace", label: "Namespace (bucket and access key id)", required: true),
            ], mutates: false),
        .init(id: "object-copy", title: "Copy an object to another key of its namespace, on the server", path: ["storage", "cp"],
            hostPlacement: .none, fields: [
                .init(id: "source", label: "Source: stado://<namespace>/<key>", required: true),
                .init(id: "target", label: "Target: stado://<namespace>/<key>, same namespace", required: true),
            ]),
        .init(id: "object-move", title: "Move an object to another key of its namespace, on the server", path: ["storage", "mv"],
            hostPlacement: .none, fields: [
                .init(id: "source", label: "Source: stado://<namespace>/<key>, removed once copied", required: true),
                .init(id: "target", label: "Target: stado://<namespace>/<key>, same namespace", required: true),
            ]),
        .init(id: "object-list", title: "List a namespace's objects", path: ["storage", "objects"],
            hostPlacement: .none, fields: [
                .init(id: "namespace", label: "Namespace", required: true),
                .init(id: "prefix", label: "Key prefix (blank lists the whole namespace)"),
            ], mutates: false),
        .init(id: "object-stat", title: "Read whether an object is present, its size, version and metadata",
            path: ["storage", "stat"], hostPlacement: .none, fields: [
                .init(id: "path", label: "Object: stado://<namespace>/<key>, or a store name such as registry.json",
                    required: true),
            ], mutates: false),
        .init(id: "object-cat", title: "Print an object's body", path: ["storage", "cat"],
            hostPlacement: .none, fields: [
                .init(id: "path", label: "Object: stado://<namespace>/<key>, or a store name such as registry.json",
                    required: true),
            ], mutates: false, jsonOutput: false),
        .init(id: "object-get", title: "Download an object into a local file", path: ["storage", "get"],
            hostPlacement: .none, fields: [
                .init(id: "uri", label: "Object: stado://<namespace>/<key>", required: true),
                .init(id: "destination", label: "Local file to write", required: true),
            ], mutates: false, jsonOutput: false),
        .init(id: "object-put", title: "Upload a local file as an object", path: ["storage", "put"],
            hostPlacement: .none, fields: [
                .init(id: "uri", label: "Object: stado://<namespace>/<key>", required: true),
                .init(id: "source", label: "Local file to upload", required: true),
                .init(id: "content-type", label: "Media type (blank keeps application/octet-stream)",
                    option: "--content-type"),
                .init(id: "if-absent", label: "Refuse to replace an existing object", option: "--if-absent", flag: true),
            ]),
        .init(id: "object-remove", title: "Remove an object", path: ["storage", "rm"],
            hostPlacement: .none, fields: [
                .init(id: "uri", label: "Object: stado://<namespace>/<key>", required: true),
            ]),
        .init(id: "object-abort-upload", title: "Discard the staged parts of an interrupted upload",
            path: ["storage", "abort-upload"], hostPlacement: .none, fields: [
                .init(id: "uri", label: "Target object the upload was writing: stado://<namespace>/<key>", required: true),
                .init(id: "dry-run", label: "Report the parts and remove nothing", option: "--dry-run", flag: true),
            ]),
        .init(id: "bucket-create", title: "Create a bucket on this host's self-hosted store", path: ["storage", "bucket", "create"],
            fields: bucketFields + bucketGrantFields),
        .init(id: "bucket-list", title: "List the buckets this host serves", path: ["storage", "bucket", "list"],
            fields: [], mutates: false),
        .init(id: "bucket-show", title: "Show one bucket this host serves", path: ["storage", "bucket", "show"],
            fields: bucketFields, mutates: false),
        .init(id: "bucket-update", title: "Replace a bucket's grant", path: ["storage", "bucket", "update"],
            fields: bucketFields + bucketGrantFields),
        .init(id: "bucket-rotate", title: "Replace a bucket's key", path: ["storage", "bucket", "rotate"],
            hostPlacement: .none, fields: bucketFields),
        .init(id: "bucket-delete", title: "Delete a bucket and its key (objects stay on disk)", path: ["storage", "bucket", "delete"],
            fields: bucketFields + [
                .init(id: "confirm", label: "Bucket name again", option: "--confirm", required: true),
                .init(id: "reload", label: "Unit to restart afterwards (optional)", option: "--reload-service"),
            ]),
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
