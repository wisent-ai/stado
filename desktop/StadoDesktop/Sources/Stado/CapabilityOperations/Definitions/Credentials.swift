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
        .init(id: "credential-get", title: "Read credential value", path: ["credentials", "get"], hostPlacement: .none, fields: [
            .init(id: "item", label: "Item identifier", required: true),
            .init(id: "field", label: "Exact field (required for delegated read)", option: "--field"),
            .init(id: "route", label: "Skarbiec route URL (delegated read)", option: "--route"),
            .init(id: "consumer", label: "Declared consumer (requires route)", option: "--consumer"),
            .init(id: "grant-file", label: "Owner-only grant file (requires route)", option: "--grant-file"),
        ], mutates: false, jsonOutput: false),
        .init(id: "item-retag", title: "Read or change item tags", path: ["credentials", "item", "retag"], fields: [
            .init(id: "item", label: "Item identifier", required: true),
            .init(id: "tags", label: "Comma-separated tags (blank reads only)", option: "--tags"),
        ]),
        .init(id: "item-signing-profile", title: "Store code-signing provisioning profiles in a signing item", path: ["credentials", "item", "signing-profile"], fields: [
            .init(id: "provider", label: "Profile provider (apple)", option: "--provider", required: true, initial: "apple"),
            .init(id: "item", label: "Signing item (for example tama-desktop-signing)", required: true),
            .init(id: "profiles", label: "FIELD=BUNDLE_ID pairs", option: "--profile", required: true, multiple: true),
            .init(id: "credentials", label: "Provider API key item", option: "--credentials", required: true),
            .init(id: "type", label: "Profile type", option: "--type", initial: "MAC_APP_DIRECT"),
            .init(id: "certificate-type", label: "Certificate type", option: "--certificate-type", initial: "DEVELOPER_ID_APPLICATION"),
        ]),
        .init(id: "database-adopt", title: "Adopt Supabase database items (all, or one named)", path: ["database", "adopt"], hostPlacement: .none, fields: [
            .init(id: "name", label: "Declared database (blank adopts every Supabase-backed one)"),
            .init(id: "project-ref", label: "Supabase project ref (first adoption of the named one)", option: "--project-ref"),
            .init(id: "password-file", label: "File holding the database password", option: "--password-file"),
            .init(id: "check", label: "Check only; write nothing and report drift", option: "--check", flag: true, initial: "true"),
        ]),
        .init(id: "database-push", title: "Push database declarations to a host", path: ["database", "push"], hostPlacement: .positional, fields: [
            .init(id: "service", label: "Unit serving the database plane", option: "--service", required: true),
            .init(id: "check", label: "Check only; write nothing and report a difference", option: "--check", flag: true, initial: "true"),
        ]),
        .init(id: "database-destroy", title: "Destroy a fleet database: its unit, its vault item, then its declaration", path: ["database", "destroy"], hostPlacement: .none, fields: [
            .init(id: "name", label: "Declared fleet database", required: true),
            .init(id: "host", label: "Host it was placed on (blank: the vault owner)", option: "--host"),
        ]),
        .init(id: "vault-sync", title: "Check or synchronize the declared vault", path: ["credentials", "vault", "sync"], fields: [
            .init(id: "check", label: "Check without replacing the vault", option: "--check", flag: true, initial: "true"),
        ]),
        .init(id: "token-sync", title: "Check or synchronize a bearer without changing grants", path: ["credentials", "token", "sync"], fields: [
            .init(id: "consumer", label: "Exact consumer identity", required: true),
            .init(id: "source-host", label: "Source registry host", option: "--from-host", required: true),
            .init(id: "source-token", label: "Owner-only bearer file on source host", option: "--source-token-file", required: true),
            .init(id: "destination-token", label: "Owner-only bearer file on selected host", option: "--token-file", required: true),
            .init(id: "check", label: "Check only; do not replace the selected host's file", option: "--check", flag: true, initial: "true"),
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
        .init(id: "grant-consolidate", title: "Merge retired grants into Stado", path: ["credentials", "grant", "consolidate"], fields: [
            .init(id: "sources", label: "Retired consumers (one per line)", option: "--from", required: true, multiple: true),
            .init(id: "token", label: "Existing Stado bearer file on selected host (absolute path)", option: "--token-file", required: true),
        ]),
        .init(id: "migrate-identities", title: "Migrate this Stado source's identity configuration", path: ["config", "migrate-identities"],
            hostPlacement: .none, jsonOutput: false),
        .init(id: "backup-audit", title: "Audit backups or reclaim verified twins", path: ["credentials", "backup", "audit"], fields: [
            .init(id: "objects", label: "Object URIs", option: "--object", multiple: true),
            .init(id: "namespaces", label: "Inventory namespaces", option: "--inventory-namespace", multiple: true),
            .init(id: "twins", label: "Select verified backup twins", option: "--reclaim-twins", flag: true),
            .init(id: "apply", label: "Apply the selected reclamation", option: "--apply", flag: true),
        ]),
        .init(id: "seed-list", title: "List authenticator seeds", path: ["credentials", "seed", "list"], fields: [
            .init(id: "item", label: "Login item (optional)", option: "--login-item"),
        ], mutates: false),
        .init(id: "seed-enrol", title: "Enrol an authenticator seed", path: ["credentials", "seed", "enrol"], fields: [
            .init(id: "item", label: "Login item", option: "--login-item", required: true),
        ]),
        .init(id: "host-config-show", title: "Read host configuration", path: ["host", "config-show"],
            hostPlacement: .positional, mutates: false),
        .init(id: "host-config-set", title: "Set host configuration value", path: ["host", "config-set"],
            hostPlacement: .positional, fields: [
                .init(id: "key", label: "Dotted configuration key", required: true),
                .init(id: "value", label: "JSON value or plain string", required: true),
                .init(id: "service", label: "Managed service to reconcile (optional)", option: "--reload-service"),
            ], jsonOutput: false),
        .init(id: "host-config-unset", title: "Remove host configuration value", path: ["host", "config-unset"],
            hostPlacement: .positional, fields: [
                .init(id: "key", label: "Dotted configuration key", required: true),
                .init(id: "service", label: "Managed service to reconcile (optional)", option: "--reload-service"),
            ], jsonOutput: false),
    ]
}
