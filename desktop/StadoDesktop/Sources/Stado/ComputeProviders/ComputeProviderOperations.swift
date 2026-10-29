import Foundation

/// The compute-provider operations the CLI has, through the selected
/// endpoint's operator command channel: which providers exist, what a
/// profile enables, their live agent machines, the preflight that checks
/// their credentials and quota, quota catalogs, increase requests and the
/// support tickets they turn into, and the writes that enable one — a setting,
/// and the account credential under its `cloud-<provider>` role. Validating
/// the profile runs behind the review dialog because the operator channel
/// does not classify `config validate` as a read.
enum NativeComputeProviderOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "capabilities", title: "Read capability families and every provider's support",
            path: ["capabilities"], hostPlacement: .none, mutates: false),
        .init(id: "capabilities-resolve", title: "Resolve the alternative that answers a product's need (inference, secrets, browser-automation, mail, transcripts), naming every one that did not",
            path: ["capabilities", "resolve"], hostPlacement: .none, fields: [
                .init(id: "family", label: "Family", required: true, choices: ["inference", "secrets", "browser-automation", "mail", "transcripts"], initial: "inference"),
                .init(id: "consumer", label: "Consumer (the product asking)", option: "--consumer", required: true),
                .init(id: "failed", label: "An alternative that refused the caller, proven again now (blank: none)", option: "--failed"),
            ]),
        .init(id: "capabilities-select", title: "Choose the order a family's alternatives are tried in",
            path: ["capabilities", "select"], hostPlacement: .none, fields: [
                .init(id: "family", label: "Family", required: true, choices: ["inference", "secrets", "browser-automation", "mail", "transcripts"], initial: "inference"),
                .init(id: "order", label: "Alternatives in order, comma-separated", option: "--order", required: true),
            ], jsonOutput: false),
        .init(id: "instances", title: "List live agent machines (one provider, or every enabled one)",
            path: ["instances", "list"], hostPlacement: .none, fields: [
                .init(id: "provider", label: "Provider (blank: every entry in providers)", option: "--provider"),
            ], mutates: false),
        .init(id: "doctor", title: "Run the deployment preflight: provider auth, quota and agent template",
            path: ["doctor"], hostPlacement: .none, mutates: false),
        .init(id: "quota", title: "Read quota and reservations per provider",
            path: ["quota", "show"], hostPlacement: .none, mutates: false, jsonOutput: false),
        .init(id: "billing", title: "Read provider credit, and whether the Azure credit gate admits a VM create or start (the floor is the operator's ruling capability:cloud@credits-floor:azure:<amount>:<currency>)",
            path: ["billing", "show"], hostPlacement: .none, mutates: false, jsonOutput: false),
        .init(id: "quota-catalog", title: "List each provider's GPU catalog",
            path: ["quota", "catalog"], hostPlacement: .none, fields: [
                .init(id: "provider", label: "Providers, comma-separated (blank: providers)", option: "--provider"),
            ], mutates: false),
        .init(id: "quota-request-list", title: "List in-flight quota requests and support conversations",
            path: ["quota", "request", "list"], hostPlacement: .none, fields: [
                .init(id: "provider", label: "Providers, comma-separated (blank: providers)", option: "--provider"),
                .init(id: "state", label: "GCP state filter (blank: all)", option: "--state"),
                .init(id: "awaiting", label: "Azure: only tickets awaiting our reply", option: "--awaiting-customer", flag: true),
            ], mutates: false),
        .init(id: "quota-request-create", title: "Request a GPU quota increase",
            path: ["quota", "request", "create"], hostPlacement: .none, fields: [
                .init(id: "accel", label: "Accelerator (blank with every family selected)"),
                .init(id: "every-family", label: "Every GPU family the catalog reports", option: "--every-family", flag: true),
                .init(id: "to", label: "New per-region limit", option: "--to", required: true),
                .init(id: "justification", label: "Reason the provider's reviewer reads", option: "--justification", required: true),
                .init(id: "region", label: "Regions, comma-separated (blank: every dispatch region)", option: "--region"),
                .init(id: "provider", label: "Providers, comma-separated (blank: providers)", option: "--provider"),
                .init(id: "email", label: "Reviewer contact email (blank: WC_QUOTA_CONTACT_EMAIL)", option: "--email"),
            ]),
        .init(id: "quota-ticket-reply", title: "Answer quota support tickets the provider waits on",
            path: ["quota", "ticket", "reply"], hostPlacement: .none, fields: [
                .init(id: "provider", label: "Provider", option: "--provider", required: true, choices: ["azure"], initial: "azure"),
                .init(id: "dry-run", label: "Print what would be sent without posting it", option: "--dry-run", flag: true),
                .init(id: "email", label: "Signature email (blank: WC_QUOTA_CONTACT_EMAIL)", option: "--email"),
            ], jsonOutput: false),
        .init(id: "quota-ticket-escalate", title: "Escalate quota tickets declined on billing",
            path: ["quota", "ticket", "escalate"], hostPlacement: .none, fields: [
                .init(id: "provider", label: "Provider", option: "--provider", required: true, choices: ["azure"], initial: "azure"),
                .init(id: "dry-run", label: "Print what would be sent without posting it", option: "--dry-run", flag: true),
                .init(id: "email", label: "Signature email (blank: WC_QUOTA_CONTACT_EMAIL)", option: "--email"),
            ], jsonOutput: false),
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
