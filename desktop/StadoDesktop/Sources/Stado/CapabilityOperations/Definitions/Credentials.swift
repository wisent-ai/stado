import Foundation

enum NativeCredentialOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "item-put", title: "Create or replace credential item", path: ["credentials", "item", "put"], fields: [
            .init(id: "item", label: "Item identifier", required: true),
            .init(id: "kind", label: "Item kind", option: "--type", required: true),
        ], payload: .standardInput(label: "Canonical credential item JSON", initial: "")),
        .init(id: "item-show", title: "Inspect credential item", path: ["credentials", "item", "show"], fields: [
            .init(id: "item", label: "Item identifier", required: true),
            .init(id: "field", label: "Field (optional)", option: "--field"),
        ], mutates: false),
        .init(id: "item-retag", title: "Read or change item tags", path: ["credentials", "item", "retag"], fields: [
            .init(id: "item", label: "Item identifier", required: true),
            .init(id: "tags", label: "Comma-separated tags (blank reads only)", option: "--tags"),
        ]),
        .init(id: "vault-sync", title: "Check or synchronize the declared vault", path: ["credentials", "vault", "sync"], fields: [
            .init(id: "check", label: "Check without replacing the vault", option: "--check", flag: true, initial: "true"),
        ]),
        .init(id: "acquisition-sync", title: "Synchronize acquisition scope catalogue", path: ["credentials", "acquisition-scopes", "sync"],
            payload: .file(option: nil, label: "Acquisition scope catalogue contents", initial: "")),
        .init(id: "grant-read", title: "Grant an exact item field read", path: ["credentials", "grant", "item-read"], fields: [
            .init(id: "consumer", label: "Consumer", required: true),
            .init(id: "item", label: "Item", required: true),
            .init(id: "field", label: "Field", option: "--field", required: true),
            .init(id: "token", label: "Existing bearer file on the target", option: "--token-file", required: true),
        ]),
        .init(id: "grant-show", title: "Inspect a consumer grant", path: ["credentials", "grant", "show"], fields: [
            .init(id: "consumer", label: "Consumer", required: true),
            .init(id: "token", label: "Bearer file on the target (optional)", option: "--token-file"),
        ], mutates: false),
        .init(id: "backup-audit", title: "Audit backups or reclaim verified twins", path: ["credentials", "backup", "audit"], fields: [
            .init(id: "objects", label: "Object URIs", option: "--object", multiple: true),
            .init(id: "namespaces", label: "Inventory namespaces", option: "--inventory-namespace", multiple: true),
            .init(id: "twins", label: "Select verified backup twins", option: "--reclaim-twins", flag: true),
            .init(id: "apply", label: "Apply the selected reclamation", option: "--apply", flag: true),
        ]),
        .init(id: "seed-freshness", title: "Inspect authenticator seed freshness", path: ["credentials", "seed-freshness"], fields: [
            .init(id: "item", label: "Login item (optional)", option: "--login-item"),
        ], mutates: false),
    ]
}
