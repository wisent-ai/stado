import SwiftUI
import WisentDesignSystem

/// Adding a machine to the fleet, starting from the question that decides
/// everything else: which way in.
///
/// There is more than one way, they are not interchangeable, and picking the
/// wrong one is what turns adding a laptop into a phone call. So this window
/// opens on the list of them, reported by the control plane rather than
/// remembered by the app, with what each one needs from the operator and what
/// the registry catalog permits. Each way in is then its own screen, because
/// each one asks for different things.
struct MachineEnrollmentView: View {
    @ObservedObject var store: MachineEnrollmentStore
    /// Names the registry and the capacity store already know. Enrollment
    /// refuses a duplicate, and it refuses it after the operator has already
    /// done the work.
    let existingNames: Set<String>
    let refresh: () async -> Void

    var body: some View {
        Group {
            switch store.flow {
            case .methods:
                EnrollmentMethodListView(store: store)
            case .invite:
                EnrollmentInviteView(store: store, existingNames: existingNames, refresh: refresh)
            case .adopt:
                EnrollmentAdoptView(store: store, existingNames: existingNames, refresh: refresh)
            case .handKey:
                MachineHandKeyEnrollmentView(
                    store: store,
                    existingNames: existingNames,
                    refresh: refresh
                )
            case .join:
                EnrollmentJoinView(store: store, refresh: refresh)
            case .declare:
                EnrollmentDeclareView(store: store)
            }
        }
        .task {
            await store.loadMethods()
        }
    }
}

/// The ways in, as this control plane reports them.
///
/// The list is read, never held: a Stado that gains a method has to show it
/// here without a new app, and a registry whose catalog forbids one has to show
/// that too. A forbidden method stays on the list, disabled, naming the field
/// that forbade it — removing it would leave the operator hunting for a door
/// that was deliberately locked.
struct EnrollmentMethodListView: View {
    @ObservedObject var store: MachineEnrollmentStore

    var body: some View {
        EnrollmentChrome(
            store: store,
            eyebrow: MachineEnrollmentFlow.methods.eyebrow,
            title: "Ways to add a machine",
            detail: "Four of them, and they differ in one thing: what you can already do to the machine you are adding. Every one of them ends the same way, with the fleet holding a private key it never sends and the machine holding the matching public key.",
            trailing: store.plan.invite.map { ("INVITATION OPEN", $0.targetName) },
            showsWaysIn: false,
            guidance: guidance,
            actions: [
                WisentAction(
                    "Read the list again",
                    symbol: "arrow.clockwise",
                    isEnabled: !store.isReadingMethods && !store.isRunning
                ) {
                    Task { await store.loadMethods(force: true) }
                },
            ]
        ) {
            if let invite = store.plan.invite {
                EnrollmentNote(
                    title: invite.isOffline
                        ? "\(invite.targetName) is invited and waiting on its owner"
                        : "An invitation is still open",
                    detail: invite.isOffline
                        ? "The fragment for it was minted from this window. Nothing reports itself in that mode: what you are waiting for is the address its owner reads back to you, and the invite screen is where you put it."
                        : "It was minted from this window and is waiting to be answered. Nothing is expected of you until the machine reports itself.",
                    actions: [
                        WisentAction("Go to it", kind: .primary) { store.open(.invite) },
                    ]
                )
            }

            if store.methods.isEmpty {
                if store.isReadingMethods {
                    WisentLoadingPanel(
                        title: "Asking this Stado which ways in it offers",
                        detail: "stado fleet methods reports each method with what it requires and whether the registry catalog for this fleet permits it."
                    )
                } else {
                    WisentEmptyPanel(
                        title: "No methods reported",
                        detail: "This app does not carry a list of its own, so there is nothing to show until the control plane answers. Nothing here is inferred from local configuration.",
                        symbol: "questionmark.folder",
                        action: WisentAction("Try again", symbol: "arrow.clockwise", kind: .primary) {
                            Task { await store.loadMethods(force: true) }
                        }
                    )
                }
            } else {
                WisentSectionBox(
                    title: "Reported by this control plane",
                    detail: "In the order Stado lists them, which is the order of least work for you.",
                    trailing: "\(store.methods.count) methods"
                ) {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(store.methods.enumerated()), id: \.element.id) { index, method in
                            if index > 0 {
                                Divider()
                            }
                            EnrollmentMethodRow(
                                title: method.name.capitalized,
                                summary: method.summary,
                                requires: method.requires,
                                provides: method.provides,
                                command: method.command,
                                refusal: method.refusal,
                                isRecommended: method.id == recommended,
                                open: { store.open(method.flow ?? .methods) }
                            )
                        }
                    }
                    .background(WisentDesign.surface, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.medium))
                    .overlay {
                        RoundedRectangle(cornerRadius: WisentDesign.Radius.medium)
                            .stroke(WisentDesign.border, lineWidth: WisentDesign.hairline)
                    }
                }
            }

            WisentSectionBox(
                title: "When nobody can reach the machine at all",
                detail: "Not a fifth method in the registry: it is the enrollment above with the key installed by hand instead of by Stado. It is the only path left for a machine no operator can open a session to and whose owner will not run a line they were sent, and it is the only one that asks you to walk somewhere."
            ) {
                VStack(alignment: .leading, spacing: 0) {
                    EnrollmentMethodRow(
                        title: MachineEnrollmentFlow.handKey.title,
                        summary: "Stado mints the pair and shows you the public half. You put it into the machine's authorized_keys yourself, then enroll by address.",
                        requires: "Physical or remote access to the machine by some other means, and an address the fleet can reach it at afterwards",
                        provides: "The same registry entry, channel, and agent as the methods above, at the cost of one walk",
                        command: store.draft.machineName.isEmpty
                            ? "stado fleet key generate NAME, then stado fleet enroll NAME --ssh DEST --bootstrap"
                            : "stado fleet key generate \(store.draft.machineName), then \(store.draft.enrollCommand)",
                        refusal: nil,
                        isRecommended: false,
                        open: { store.open(.handKey) }
                    )
                }
                .background(WisentDesign.surface, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.medium))
                .overlay {
                    RoundedRectangle(cornerRadius: WisentDesign.Radius.medium)
                        .stroke(WisentDesign.border, lineWidth: WisentDesign.hairline)
                }
            }

            if !store.draft.isEmpty, !store.draft.isEnrolled {
                EnrollmentNote(
                    title: store.draft.machineName.isEmpty
                        ? "An unfinished attempt is kept"
                        : "The unfinished attempt at \(store.draft.machineName) is kept",
                    detail: store.draft.hasKey
                        ? "A key pair for it already exists in the credential store, so continuing costs nothing and starting over leaves that pair behind."
                        : "Nothing has been minted or written for it yet, so discarding it costs nothing either.",
                    actions: [
                        WisentAction("Discard it", kind: .plain, isEnabled: !store.isRunning) {
                            store.startAnother()
                        },
                    ]
                )
            }
        }
    }

    /// The first method the operator can actually use.
    ///
    /// The list arrives in order of least work, so the first usable row is a
    /// recommendation the control plane already made. It gets the one filled
    /// button on the screen: four identical primary buttons are no hierarchy at
    /// all, and the choice still belongs to the operator.
    private var recommended: String? {
        store.methods.first { $0.isOpen }?.id
    }

    private var guidance: String {
        if store.methods.isEmpty {
            return "The list is read from the control plane every time this window opens. Nothing on this screen is a local default."
        }
        let refused = store.methods.filter { !$0.allowed }.map(\.name)
        guard !refused.isEmpty else {
            return "This fleet's registry permits every method Stado offers."
        }
        return "This fleet's registry catalog forbids \(Self.list(refused)). Those rows stay on the list, disabled, naming the field that forbade them."
    }

    /// "a", "a and b", "a, b and c" — a list an operator reads as a sentence.
    private static func list(_ values: [String]) -> String {
        guard let last = values.last else { return "" }
        guard values.count > 1 else { return last }
        return "\(values.dropLast().joined(separator: ", ")) and \(last)"
    }
}
