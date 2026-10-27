import SwiftUI
import WisentDesignSystem

/// Trading this fleet's idle capacity with other Stado fleets: the same
/// capability as `stado market peer` and `stado market --provider stado`.
/// Every control runs the command a terminal would, behind a confirmation
/// that shows it, and every refusal is the command's own sentence.
struct PeerFleetsPanel: View {
    @ObservedObject var store: PeerFleetsStore

    @State private var form = PeerFleetForm()
    @State private var editing = false
    @State private var clearAuthority = false
    @State private var showingForm = false
    @State private var pending: PendingPeerAction?
    @State private var priceGPU = EarningConstants.defaultPriceGPU

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            HStack {
                Text("Other Stado fleets").font(WisentTypeScale.section())
                Spacer()
                Button("Declare a peer…") {
                    form = PeerFleetForm()
                    editing = false
                    clearAuthority = false
                    showingForm = true
                }
                .disabled(store.isWorking)
            }
            Text("While our queue is idle, a host can run a Stado worker against a peer fleet's queue. The peer's jobs are counted in the peer's own records at the price this host was offered at.")
                .font(WisentTypeScale.caption())
            if let problem = store.problem {
                WisentErrorBanner(title: "stado market refused", detail: problem)
            }
            if let outcome = store.actionOutcome {
                Text(outcome).font(WisentTypeScale.identifier()).textSelection(.enabled)
            }
            if store.peers.isEmpty {
                Text("No peer fleet is declared.").font(WisentTypeScale.caption())
            }
            ForEach(store.peers) { peer in
                peerRow(peer)
            }
            HStack(spacing: WisentDesign.Space.x3) {
                TextField("Offer price per GPU-hour", value: $priceGPU, format: .number)
                    .frame(width: EarningConstants.priceFieldWidth)
                Text("the price Offer uses").font(WisentTypeScale.caption())
            }
            PeerWorkHereSection()
        }
        .task { await store.refresh() }
        .sheet(isPresented: $showingForm) { formSheet }
        .sheet(item: $pending) { action in confirmation(action) }
    }

    @ViewBuilder
    private func peerRow(_ peer: PeerFleet) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text(peer.name).font(WisentTypeScale.section())
            LabeledContent("Origin", value: peer.origin)
            LabeledContent("Namespace", value: peer.namespace)
            LabeledContent("Credential", value: "\(peer.credentialRole)/token")
            if let caFile = peer.caFile {
                LabeledContent("Authority", value: caFile)
            }
            if let readiness = store.readiness[peer.name] {
                Text(readiness.summary)
                    .foregroundStyle(readiness.lending ? WisentDesign.success : WisentDesign.danger)
                    .textSelection(.enabled)
                ForEach(readiness.remedy, id: \.self) { line in
                    Text(line).font(WisentTypeScale.identifier()).textSelection(.enabled)
                }
            }
            if let ledger = store.ledgers[peer.name] {
                ForEach(ledger.hosts) { line in
                    LabeledContent(
                        line.host,
                        value: "\(line.listed ? "offered" : "withdrawn") at $\(line.priceGpu)/GPU-hour · \(line.state) · lent \(line.lent.seconds)s = $\(line.amountUsd)"
                    )
                }
            }
            if let problem = store.peerProblems[peer.name] {
                Text(problem).foregroundStyle(WisentDesign.danger).textSelection(.enabled)
            }
            PeerFleetMonitorSection(peer: peer.name, priceGPU: priceGPU)
            HStack(spacing: WisentDesign.Space.x3) {
                Button("Offer this host…") { pending = .offer(peer: peer.name, priceGPU: priceGPU) }
                Button("Withdraw…") { pending = .withdraw(peer: peer.name) }
                Button("Edit…") {
                    form = PeerFleetForm(peer: peer)
                    editing = true
                    clearAuthority = false
                    showingForm = true
                }
                Button("Retire…", role: .destructive) { pending = .remove(peer: peer.name) }
            }
            .disabled(store.isWorking)
        }
    }

    private var formSheet: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            Text(editing ? "Change peer \(form.name)" : "Declare a peer fleet")
                .font(WisentTypeScale.section())
            TextField("Name", text: $form.name).disabled(editing)
            TextField("Object API origin", text: $form.origin)
            TextField("Queue namespace", text: $form.namespace)
            TextField("Credential role (its item's token field holds the peer's bearer)", text: $form.credentialRole)
            TextField("PEM authority file (empty for the system roots)", text: $form.caFile)
            if editing {
                Toggle("Trust the origin through the system roots again", isOn: $clearAuthority)
            }
            Text(StadoCLI.commandLine(
                editing
                    ? PeerFleetsStore.editArguments(form, clearAuthority: clearAuthority)
                    : PeerFleetsStore.addArguments(form)
            ))
            .font(WisentTypeScale.identifier())
            .textSelection(.enabled)
            HStack {
                Button("Cancel") { showingForm = false }
                Button(editing ? "Change" : "Declare") {
                    showingForm = false
                    let submitted = form
                    let clearing = clearAuthority
                    let changing = editing
                    Task {
                        if changing {
                            await store.edit(submitted, clearAuthority: clearing)
                        } else {
                            await store.add(submitted)
                        }
                    }
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(WisentDesign.Space.x4)
    }

    private func confirmation(_ action: PendingPeerAction) -> WisentDecisionDialog {
        WisentDecisionDialog(
            tone: action.tone,
            title: action.title,
            lines: [action.consequence],
            listing: ["command: " + StadoCLI.commandLine(action.arguments)],
            footnote: "Runs the same command a terminal would.",
            actions: [
                WisentAction("Cancel", kind: .secondary) { pending = nil },
                WisentAction(action.verb, kind: .primary) {
                    pending = nil
                    Task { await run(action) }
                },
            ]
        )
    }

    private func run(_ action: PendingPeerAction) async {
        switch action {
        case .offer(let peer, let priceGPU): await store.offer(peer: peer, priceGPU: priceGPU)
        case .withdraw(let peer): await store.withdraw(peer: peer)
        case .remove(let peer): await store.remove(peer: peer)
        }
    }
}

extension PeerFleetForm {
    /// The form holding a declared peer, for a change.
    init(peer: PeerFleet) {
        self.init(name: peer.name, origin: peer.origin, namespace: peer.namespace,
                  credentialRole: peer.credentialRole)
        if let caFile = peer.caFile { self.caFile = caFile }
    }
}

/// A peer action waiting for the operator's confirmation.
enum PendingPeerAction: Identifiable, Equatable {
    case offer(peer: String, priceGPU: Double)
    case withdraw(peer: String)
    case remove(peer: String)

    var id: String { arguments.joined(separator: " ") }

    var arguments: [String] {
        switch self {
        case .offer(let peer, let priceGPU): return PeerFleetsStore.offerArguments(peer: peer, priceGPU: priceGPU)
        case .withdraw(let peer): return PeerFleetsStore.withdrawArguments(peer: peer)
        case .remove(let peer): return PeerFleetsStore.removeArguments(peer: peer)
        }
    }

    var title: String {
        switch self {
        case .offer(let peer, let priceGPU): return "Offer this host to \(peer) at $\(priceGPU)/GPU-hour?"
        case .withdraw(let peer): return "Withdraw this host from \(peer)?"
        case .remove(let peer): return "Retire peer \(peer)?"
        }
    }

    var consequence: String {
        switch self {
        case .offer:
            return "The follow loop that holds this peer lends the host once our queue has been idle for its window. Without such a loop the command refuses."
        case .withdraw:
            return "No new lending session starts. A running session is drained: the peer's jobs it runs finish first."
        case .remove:
            return "The declaration and every host's offer to the peer are removed. Refused while a loop on this host holds the peer."
        }
    }

    var verb: String {
        switch self {
        case .offer: return "Offer"
        case .withdraw: return "Withdraw"
        case .remove: return "Retire"
        }
    }

    /// How the confirmation dialog colours this action.
    var tone: WisentTone {
        switch self {
        case .offer: return .warning
        case .withdraw, .remove: return .danger
        }
    }
}
