import Foundation

/// The compute-provider operations the CLI has, through the selected
/// endpoint's operator command channel: which providers exist, what a
/// profile enables, their live agent machines, the preflight that checks
/// their credentials and quota, and the writes that enable one — a setting,
/// and the account credential under its `cloud-<provider>` role. Validating
/// the profile runs behind the review dialog because the operator channel
/// does not classify `config validate` as a read.
enum NativeComputeProviderOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "capabilities", title: "Read capability families and every provider's support",
            path: ["capabilities"], hostPlacement: .none, mutates: false),
        .init(id: "instances", title: "List live agent machines (one provider, or every enabled one)",
            path: ["instances", "list"], hostPlacement: .none, fields: [
                .init(id: "provider", label: "Provider (blank: every entry in providers)", option: "--provider"),
            ], mutates: false),
        .init(id: "doctor", title: "Run the deployment preflight: provider auth, quota and agent template",
            path: ["doctor"], hostPlacement: .none, mutates: false),
        .init(id: "quota", title: "Read quota and reservations per provider",
            path: ["quota", "show"], hostPlacement: .none, mutates: false, jsonOutput: false),
        .init(id: "config-validate", title: "Validate the selected profile, required provider settings included",
            path: ["config", "validate"], hostPlacement: .none, jsonOutput: false),
        .init(id: "config-set", title: "Set a provider setting or the providers list",
            path: ["config", "set"], hostPlacement: .none, fields: [
                .init(id: "key", label: "Dotted key (for example lambda.region, or providers)", required: true),
                .init(id: "value", label: "JSON value or plain string", required: true),
            ], jsonOutput: false),
        .init(id: "config-unset", title: "Remove a provider setting",
            path: ["config", "unset"], hostPlacement: .none, fields: [
                .init(id: "key", label: "Dotted key", required: true),
            ], jsonOutput: false),
        .init(id: "credential-put", title: "Store a provider's account credential under role cloud-<provider>",
            path: ["credentials", "item", "put"], hostPlacement: .none, fields: [
                .init(id: "host", label: "Host that owns the vault", option: "--host", required: true),
                .init(id: "role", label: "Role (cloud-lambda, cloud-voltage-park, …)", option: "--role", required: true),
                .init(id: "type", label: "Item kind", option: "--type", required: true, initial: "api-key"),
            ], payload: .standardInput(label: "Credential JSON object with the provider's fields", initial: "")),
    ]
}
