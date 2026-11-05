import SwiftUI
import WisentDesignSystem

/// The canonical fleet policy, target by target, and the one field of it this
/// console may write.
///
/// The screen's own parts live in `Registry/`: the facet rail and the three
/// zones in `Registry/RegistryFacets.swift`, the rows in
/// `Registry/RegistryTable.swift`, the policy pane and its write buttons in
/// `Registry/RegistryInspector.swift`, the confirmations in
/// `Registry/RegistryDialogs.swift`, and the filter and the pending write in
/// `Registry/RegistryPolicyDecision.swift`. The stored state stays here,
/// because `@State` belongs to the view rather than to any one of its
/// sections.
struct RegistryView: View {
    @ObservedObject var fleetStore: FleetControlStore
    let scope: String

    @State var facet: RegistryFacet = .all
    @State var selection: String?
    @State var decision: PolicyDecision?

    var body: some View {
        WisentScreen(
            title: "Registry",
            scope: scope,
            freshness: freshness,
            actions: [
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !fleetStore.isRefreshing) {
                    Task { await fleetStore.refresh() }
                }
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing:
                0) {
                WisentSectionBox(
                    title: "Registry documents",
                    detail: "Read the selected endpoint's registry, or validate and publish a document you supply. No source-checkout snapshot is selected for you."
                ) {
                    NativeCapabilityActions(host: "", fleet: fleetStore, operations: NativeRegistryOperations.all)
                }
                .padding(WisentDesign.Space.x4)

                if let message = fleetStore.errorMessage {
                    WisentErrorBanner(
                        title: fleetStore.isShowingStalePolicy
                            ? "Refresh failed — the policy below is the last projection that was read"
                            : "Canonical fleet policy unavailable",
                        detail: message,
                        action: WisentAction("Retry", symbol: "arrow.clockwise") {
                            Task { await fleetStore.refresh() }
                        }
                    )
                    .padding(WisentDesign.Space.x4)
                }

                if fleetStore.policy != nil {
                    zones
                } else {
                    placeholder
                        .padding(WisentDesign.Space.x6)
                    Spacer(minLength:
                        0)
                }

                WisentMutationBar(outcome: fleetStore.mutation) { fleetStore.clearMutation() }
                    .padding(.horizontal, WisentDesign.Space.x4)
                    .padding(.bottom, fleetStore.mutation == .idle ? 0 : WisentDesign.Space.x3)
            }
        }
        .sheet(item: $decision) { pending in
            dialog(for: pending)
        }
    }

    private var freshness: String {
        guard let policy = fleetStore.policy else {
            return fleetStore.isConfigured ? "Not read yet" : "Not configured"
        }
        return "Generation \(policy.generation) · read \(ConsoleFormat.relative(fleetStore.lastUpdated))"
    }

    @ViewBuilder
    private var placeholder: some View {
        if fleetStore.isRefreshing {
            Group {
                let loadingTitle = "Reading canonical fleet policy"
                WisentSectionBox(title: loadingTitle, detail: "The dashboard projects three policy fields per target. The Hosts screen reads control routes separately through stado host link; SSH credentials never cross this projection.") {
                    WisentSkeletonList(label: loadingTitle)
                }
            }
        } else if !fleetStore.isConfigured {
            WisentEmptyPanel(
                title: "No Stado endpoint",
                detail: "Choose a source in the sidebar to read the canonical policy this fleet runs on.",
                symbol: "book.closed"
            )
        } else {
            WisentEmptyPanel(
                title: "No policy projection",
                detail: "The dashboard has not returned the canonical registry projection. Nothing about fleet policy is assumed while it is missing.",
                symbol: "book.closed",
                action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary) {
                    Task { await fleetStore.refresh() }
                }
            )
        }
    }

    // MARK: Values

    var targets: [FleetPolicyTarget] {
        let targets = fleetStore.targets
        switch facet {
        case .all: return targets
        case .pinned: return targets.filter { $0.pinnedOnly == true }
        case .open: return targets.filter { $0.pinnedOnly != true }
        }
    }

    func badges(for target: FleetPolicyTarget) -> [(String, WisentTone)] {
        target.pinnedOnly == true ? [("Routed only", .neutral)] : []
    }
}

enum NativeRegistryOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "registry-document-read", title: "Read the canonical registry and its generation",
              path: ["registry", "pull"], hostPlacement: .none,
              fixedArguments: ["--with-generation"], mutates: false, jsonOutput: false),
        .init(id: "registry-document-history", title: "Show who changed the registry, or one path of it",
              path: ["registry", "history"], hostPlacement: .none, fields: [
                  .init(id: "path", label: "Dotted path, such as public_origins.release-object (empty: every write)", option: "--path"),
              ], mutates: false, jsonOutput: false),
        .init(id: "cost-host", title: "What each hour of an owned host cost, at its declared power tariff",
              path: ["cost", "host"], hostPlacement: .none, fields: [
                  .init(id: "target", label: "Registry target", required: true),
                  .init(id: "since-hours", label: "Hours back (the beacon keeps the last day)", option: "--since-hours", initial: "24"),
              ], mutates: false, jsonOutput: false),
        .init(id: "data-export", title: "Export the fleet's measurements as a dataset naming no host, customer or job",
              path: ["data", "export"], hostPlacement: .none, fields: [
                  .init(id: "kind", label: "Kind", option: "--kind", required: true, choices: ["hours", "capacity", "prices"]),
                  .init(id: "since-days", label: "Days back", option: "--since-days", initial: "30"),
                  .init(id: "out", label: "Empty or new output directory", option: "--out", required: true),
                  .init(id: "key-role", label: "Credential role holding the export key", option: "--key-role", required: true),
              ], jsonOutput: false),
        .init(id: "demand-event-list", title: "Grid demand-response events, newest first",
              path: ["demand", "event", "list"], hostPlacement: .none, fields: [], mutates: false, jsonOutput: false),
        .init(id: "demand-event-add", title: "Record a demand-response event: named hosts shed interruptible load in its window",
              path: ["demand", "event", "add"], hostPlacement: .none, fields: [
                  .init(id: "id", label: "Event id", required: true),
                  .init(id: "program", label: "Program", option: "--program", required: true),
                  .init(id: "host", label: "Registry target that sheds load", option: "--host", required: true, multiple: true),
                  .init(id: "starts-at", label: "Window start (RFC 3339)", option: "--starts-at", required: true),
                  .init(id: "ends-at", label: "Window end (RFC 3339)", option: "--ends-at", required: true),
                  .init(id: "source", label: "Where the event was read", option: "--source", required: true),
              ], jsonOutput: false),
        .init(id: "demand-event-withdraw", title: "Withdraw a demand-response event",
              path: ["demand", "event", "withdraw"], hostPlacement: .none, fields: [
                  .init(id: "id", label: "Event id", required: true),
              ], jsonOutput: false),
        .init(id: "demand-event-paid", title: "Record what a demand-response program paid for an event",
              path: ["demand", "event", "paid"], hostPlacement: .none, fields: [
                  .init(id: "id", label: "Event id", required: true),
                  .init(id: "amount", label: "Amount", option: "--amount", required: true),
                  .init(id: "currency", label: "Currency", option: "--currency", required: true),
                  .init(id: "source", label: "Where the payment was read", option: "--source", required: true),
              ], jsonOutput: false),
        .init(id: "demand-event-report", title: "What a demand-response event shed per host, and what it paid",
              path: ["demand", "event", "report"], hostPlacement: .none, fields: [
                  .init(id: "id", label: "Event id", required: true),
              ], mutates: false, jsonOutput: false),
        .init(id: "inference-callers-list", title: "Outside callers of an inference deployment, with their limits and prices",
              path: ["inference", "callers", "list"], hostPlacement: .none, fields: [
                  .init(id: "deployment", label: "Deployment", required: true),
              ], mutates: false, jsonOutput: false),
        .init(id: "inference-callers-add", title: "Issue a key to an outside caller of an inference deployment",
              path: ["inference", "callers", "add"], hostPlacement: .none, fields: [
                  .init(id: "deployment", label: "Deployment", required: true),
                  .init(id: "caller", label: "Caller name", option: "--caller", required: true),
                  .init(id: "requests-per-minute", label: "Requests per minute", option: "--requests-per-minute", required: true),
                  .init(id: "tokens-per-day", label: "Tokens per UTC day", option: "--tokens-per-day", required: true),
                  .init(id: "usd-per-million-prompt", label: "USD per million prompt tokens", option: "--usd-per-million-prompt"),
                  .init(id: "usd-per-million-generated", label: "USD per million generated tokens", option: "--usd-per-million-generated"),
              ], jsonOutput: false),
        .init(id: "inference-callers-revoke", title: "Revoke an outside caller's key",
              path: ["inference", "callers", "revoke"], hostPlacement: .none, fields: [
                  .init(id: "deployment", label: "Deployment", required: true),
                  .init(id: "caller", label: "Caller name", option: "--caller", required: true),
              ], jsonOutput: false),
        .init(id: "inference-callers-usage", title: "Each outside caller's requests, tokens, refusals and charge per hour",
              path: ["inference", "callers", "usage"], hostPlacement: .none, fields: [
                  .init(id: "deployment", label: "Deployment", required: true),
                  .init(id: "caller", label: "Caller (blank: every caller)", option: "--caller"),
                  .init(id: "since-hours", label: "Hours back", option: "--since-hours", initial: "24"),
              ], mutates: false, jsonOutput: false),
        .init(id: "inference-offer-set", title: "Sell one call of a deployment to a caller with no key",
              path: ["inference", "offer", "set"], hostPlacement: .none, fields: [
                  .init(id: "deployment", label: "Deployment", required: true),
                  .init(id: "usd-per-million-prompt", label: "USD per million prompt tokens", option: "--usd-per-million-prompt", required: true),
                  .init(id: "usd-per-million-generated", label: "USD per million generated tokens", option: "--usd-per-million-generated", required: true),
                  .init(id: "max-prompt-tokens", label: "Most prompt tokens per call", option: "--max-prompt-tokens", required: true),
                  .init(id: "max-generated-tokens", label: "Most generated tokens per call", option: "--max-generated-tokens", required: true),
                  .init(id: "quote-seconds", label: "Seconds a quote holds", option: "--quote-seconds", required: true),
                  .init(id: "method", label: "Payment method NAME=VERIFY_URL[,VAULT_ITEM]", option: "--method", required: true, multiple: true),
              ], jsonOutput: false),
        .init(id: "inference-offer-show", title: "A deployment's pay-per-call offer and the most one call costs",
              path: ["inference", "offer", "show"], hostPlacement: .none, fields: [
                  .init(id: "deployment", label: "Deployment", required: true),
              ], mutates: false, jsonOutput: false),
        .init(id: "inference-offer-withdraw", title: "Stop selling calls without a key",
              path: ["inference", "offer", "withdraw"], hostPlacement: .none, fields: [
                  .init(id: "deployment", label: "Deployment", required: true),
              ], jsonOutput: false),
        .init(id: "inference-offer-settlements", title: "Each paid call: paid, outcome, tokens, cost and refund due",
              path: ["inference", "offer", "settlements"], hostPlacement: .none, fields: [
                  .init(id: "deployment", label: "Deployment", required: true),
                  .init(id: "since-hours", label: "Hours back", option: "--since-hours", initial: "24"),
              ], mutates: false, jsonOutput: false),
        .init(id: "market-earnings", title: "What a host earned on Vast.ai per day, beside its cost and margin",
              path: ["market", "earnings"], hostPlacement: .none, fields: [
                  .init(id: "target", label: "Registry target", option: "--target", required: true),
                  .init(id: "since-days", label: "Days back", option: "--since-days", initial: "7"),
              ], fixedArguments: ["--provider", "vast"], mutates: false, jsonOutput: false),
        .init(id: "market-compare", title: "Each of a host's uses per day beside its energy cost and margin",
              path: ["market", "compare"], hostPlacement: .none, fields: [
                  .init(id: "target", label: "Registry target", option: "--target", required: true),
                  .init(id: "since-days", label: "Days back", option: "--since-days", initial: "7"),
              ], mutates: false, jsonOutput: false),
        .init(id: "market-decisions", title: "Each board decision a host's agent made, with every candidate's value",
              path: ["market", "decisions"], hostPlacement: .none, fields: [
                  .init(id: "target", label: "Worker name", option: "--target", required: true),
                  .init(id: "since-hours", label: "Hours back", option: "--since-hours", initial: "24"),
              ], mutates: false, jsonOutput: false),
        .init(id: "market-handtohuman-list", title: "Offer this host's compute or egress on HandToHuman",
              path: ["market", "list"], hostPlacement: .none, fields: [
                  .init(id: "resource", label: "What is offered", option: "--resource", required: true, choices: ["gpu", "egress"]),
                  .init(id: "price-gpu", label: "USD per GPU-hour or gigabyte", option: "--price-gpu", required: true),
                  .init(id: "units", label: "GPU-hours or gigabytes offered", option: "--units", required: true),
              ], fixedArguments: ["--provider", "handtohuman"], jsonOutput: false),
        .init(id: "market-handtohuman-unlist", title: "Withdraw this host's HandToHuman offer",
              path: ["market", "unlist"], hostPlacement: .none, fields: [
                  .init(id: "resource", label: "What was offered", option: "--resource", required: true, choices: ["gpu", "egress"]),
              ], fixedArguments: ["--provider", "handtohuman"], jsonOutput: false),
        .init(id: "market-handtohuman-earnings", title: "What this host's HandToHuman offer sold and was paid",
              path: ["market", "earnings"], hostPlacement: .none, fields: [
                  .init(id: "resource", label: "What was offered", option: "--resource", required: true, choices: ["gpu", "egress"]),
                  .init(id: "target", label: "Registry target", option: "--target", required: true),
              ], fixedArguments: ["--provider", "handtohuman"], mutates: false, jsonOutput: false),
        .init(id: "market-sources", title: "Every place a fleet host can earn, and which Stado can join",
              path: ["market", "sources"], hostPlacement: .none, fields: [
                  .init(id: "check", label: "Ask each source's page whether it still answers", option: "--check", flag: true),
                  .init(id: "target", label: "Registry target: each source's earnings per board-hour on it", option: "--target"),
              ], mutates: false, jsonOutput: false),
        .init(id: "market-measure", title: "What Vast renters pay for a board model now",
              path: ["market", "measure"], hostPlacement: .none, fields: [
                  .init(id: "gpu", label: "Board model as Vast spells it", option: "--gpu", required: true),
              ], fixedArguments: ["--provider", "vast"], jsonOutput: false),
        .init(id: "market-serve-or-rent", title: "Is a host's model worth more than renting its board?",
              path: ["market", "serve-or-rent"], hostPlacement: .none, fields: [
                  .init(id: "target", label: "Registry target", option: "--target", required: true),
                  .init(id: "outside-model", label: "Outside model as OpenRouter lists it", option: "--outside-model", required: true),
                  .init(id: "gpu", label: "Board model as Vast spells it", option: "--gpu", required: true),
                  .init(id: "since-hours", label: "Hours back", option: "--since-hours", initial: "24"),
              ], fixedArguments: ["--provider", "vast"], mutates: false, jsonOutput: false),
        .init(id: "market-node-list", title: "Network nodes this fleet runs on idle hosts, with each loop's last transition",
              path: ["market", "node", "list"], hostPlacement: .none, fields: [], mutates: false, jsonOutput: false),
        .init(id: "market-node-add", title: "Declare a network's node: the service that earns, its host, its source and what it holds",
              path: ["market", "node", "add"], hostPlacement: .none, fields: [
                  .init(id: "name", label: "Node name", required: true),
                  .init(id: "source", label: "Source id from market sources", option: "--source", required: true),
                  .init(id: "target", label: "Registry target", option: "--target", required: true),
                  .init(id: "service", label: "Declared service that runs the node", option: "--service", required: true),
                  .init(id: "cpu-cores", label: "Cores it holds", option: "--cpu-cores", required: true),
                  .init(id: "ram-gb", label: "GiB of memory it holds", option: "--ram-gb", required: true),
                  .init(id: "vram-gb", label: "GiB of board memory it holds", option: "--vram-gb", required: true),
                  .init(id: "disk-gb", label: "GiB of disk its data may take", option: "--disk-gb", initial: "0"),
                  .init(id: "deployment", label: "Inference deployment it serves (inference networks)", option: "--deployment"),
                  .init(id: "min-download-mbps", label: "Download the network requires (Mbit/s)", option: "--min-download-mbps"),
                  .init(id: "min-upload-mbps", label: "Upload the network requires (Mbit/s)", option: "--min-upload-mbps"),
                  .init(id: "requirements-source", label: "Network page stating those requirements", option: "--requirements-source"),
              ], jsonOutput: false),
        .init(id: "market-node-edit", title: "Offer or withdraw a network node, or change what it holds",
              path: ["market", "node", "edit"], hostPlacement: .none, fields: [
                  .init(id: "name", label: "Node name", required: true),
                  .init(id: "listed", label: "Offered", option: "--listed", choices: ["true", "false"]),
                  .init(id: "cpu-cores", label: "Cores it holds", option: "--cpu-cores"),
                  .init(id: "ram-gb", label: "GiB of memory it holds", option: "--ram-gb"),
                  .init(id: "vram-gb", label: "GiB of board memory it holds", option: "--vram-gb"),
                  .init(id: "disk-gb", label: "GiB of disk its data may take", option: "--disk-gb"),
                  .init(id: "deployment", label: "Inference deployment it serves (empty: none)", option: "--deployment"),
                  .init(id: "min-download-mbps", label: "Download the network requires (Mbit/s; replaces the set)", option: "--min-download-mbps"),
                  .init(id: "min-upload-mbps", label: "Upload the network requires (Mbit/s; replaces the set)", option: "--min-upload-mbps"),
                  .init(id: "requirements-source", label: "Network page stating those requirements", option: "--requirements-source"),
              ], jsonOutput: false),
        .init(id: "market-node-remove", title: "Retire a network node that is not running",
              path: ["market", "node", "remove"], hostPlacement: .none, fields: [
                  .init(id: "name", label: "Node name", required: true),
              ], jsonOutput: false),
        .init(id: "market-node-preview", title: "What a node's follow loop would do on its next poll",
              path: ["market", "node", "follow"], hostPlacement: .none, fields: [
                  .init(id: "name", label: "Node name", required: true),
                  .init(id: "idle-window-s", label: "Idle seconds before it starts", option: "--idle-window-s", initial: "0"),
              ], fixedArguments: ["--dry-run", "--once"], mutates: false, jsonOutput: false),
        .init(id: "market-maintenance", title: "Tell Vast renters this host goes down for maintenance",
              path: ["market", "maintenance"], hostPlacement: .none, fields: [
                  .init(id: "in-minutes", label: "Minutes until the host goes down", option: "--in-minutes", initial: "0"),
                  .init(id: "hours", label: "Hours it stays down", option: "--hours", required: true),
                  .init(id: "category", label: "Why", option: "--category", required: true,
                        choices: ["power", "internet", "disk", "gpu", "software", "other"]),
              ], fixedArguments: ["--provider", "vast"]),
        .init(id: "host-reboot", title: "Reboot a host, telling its Vast renters first",
              path: ["host", "reboot"], hostPlacement: .none, fields: [
                  .init(id: "target", label: "Registry target", required: true),
                  .init(id: "maintenance-hours", label: "Hours the announced window lasts", option: "--maintenance-hours", initial: "0.5"),
              ], jsonOutput: false),
        .init(id: "registry-document-validate", title: "Validate a supplied registry document",
              path: ["registry", "validate"], hostPlacement: .none,
              payload: .file(option: nil, label: "Registry JSON document", initial: ""),
              mutates: false, jsonOutput: false),
        .init(id: "registry-document-push", title: "Replace the registry from a supplied document",
              path: ["registry", "push"], hostPlacement: .none, fields: [
                  .init(id: "generation", label: "Generation read before editing", option: "--if-generation"),
                  .init(id: "force", label: "Authorize key removal or generation rollback", option: "--force", flag: true),
                  .init(id: "empty", label: "Authorize removal of every target", option: "--allow-empty-fleet", flag: true),
              ], fixedArguments: ["-"],
              payload: .standardInput(label: "Registry JSON document", initial: "")),
    ]
}
