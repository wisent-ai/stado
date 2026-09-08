import SwiftUI
import WisentDesignSystem

/// Which declared route is being added or changed.
private struct HostConnectionPathEdit: Identifiable {
    let existing: HostConnectionPathProbe?

    var id: String { existing?.name ?? "new-connection-path" }
}

/// Ordered host-control routes and their live SSH probe answers.
///
/// The registry mutation and the post-write probe stay in one sheet: a saved
/// address that did not answer is not presented as a completed repair.
struct HostConnectionPathsSheet: View {
    let host: String
    @ObservedObject var linkStore: HostLinkStore
    @ObservedObject var store: HostConnectionPathStore
    let refresh: () async -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var editor: HostConnectionPathEdit?
    @State private var pendingRemoval: HostConnectionPathProbe?

    private var link: HostLink? {
        linkStore.link(for: host)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            header
            routes
            WisentMutationBar(outcome: store.mutation) { store.clearMutation() }
            footer
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 720)
        .background(WisentDesign.canvas)
        .task {
            if link == nil {
                await refresh()
            }
        }
        .sheet(item: $editor) { edit in
            HostConnectionPathEditor(
                host: host,
                existing: edit.existing,
                store: store,
                refresh: refresh
            )
        }
        .sheet(item: $pendingRemoval) { path in
            removalDialog(path)
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("Host-control routes for \(host)")
                .font(WisentTypography.heading(17))
                .foregroundStyle(WisentDesign.ink)
            Text("Stado probes every declared SSH route without changing the host, then runs a real operation once through the first route that answered. The order here is the order it tries.")
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    @ViewBuilder
    private var routes: some View {
        WisentSectionBox(
            title: "Preferred route and fallbacks",
            detail: "The preferred route is primary. Fallback priority starts at 1 and is read from top to bottom."
        ) {
            VStack(alignment: .leading, spacing: 0) {
                if let error = link?.connectionProbeError, !error.isEmpty {
                    WisentAlertPanel(
                        tone: .danger,
                        title: "The declared routes could not be probed",
                        detail: error
                    )
                } else if let link, !link.connectionPaths.isEmpty {
                    ForEach(link.connectionPaths) { path in
                        routeRow(path, selected: path.name == link.selectedConnection)
                        if path.id != link.connectionPaths.last?.id {
                            Divider()
                        }
                    }
                } else if linkStore.isRefreshing {
                    WisentLoadingPanel(
                        title: "Probing \(host)'s routes",
                        detail: HostLinkStore.commandLine(host: host)
                    )
                } else {
                    WisentEmptyPanel(
                        title: "No host-control route was reported",
                        detail: "Add the preferred primary route or refresh the host link reading.",
                        symbol: "network"
                    )
                }
            }
        }
    }

    private func routeRow(_ path: HostConnectionPathProbe, selected: Bool) -> some View {
        HStack(alignment: .center, spacing: WisentDesign.Space.x3) {
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: WisentDesign.Space.x2) {
                    Text(path.name)
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.ink)
                    if path.name == "primary" {
                        Text("PREFERRED")
                            .font(WisentTypeScale.eyebrow())
                            .foregroundStyle(WisentDesign.muted)
                    }
                    if selected {
                        Text("SELECTED")
                            .font(WisentTypeScale.eyebrow())
                            .foregroundStyle(WisentTone.success.color)
                    }
                }
                Text(path.destination)
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(WisentDesign.secondary)
                    .textSelection(.enabled)
                Text(routeAnswer(path))
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(path.reachable ? WisentTone.success.color : WisentTone.danger.color)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Menu {
                Button("Edit…") {
                    store.clearMutation()
                    editor = HostConnectionPathEdit(existing: path)
                }
                if path.name != "primary" {
                    Button("Remove…", role: .destructive) {
                        pendingRemoval = path
                    }
                }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .accessibilityLabel("Actions for \(path.name)")
            .disabled(store.mutation.isWorking || linkStore.isRefreshing)
        }
        .padding(.vertical, WisentDesign.Space.x3)
    }

    private func routeAnswer(_ path: HostConnectionPathProbe) -> String {
        if path.reachable { return "SSH probe answered" }
        if let error = path.error, !error.isEmpty { return "SSH probe did not answer — \(error)" }
        return "SSH probe did not answer"
    }

    private var footer: some View {
        HStack(spacing: WisentDesign.Space.x2) {
            WisentActionButton(
                action: WisentAction(
                    "Add route…",
                    symbol: "plus",
                    isEnabled: !store.mutation.isWorking
                ) {
                    store.clearMutation()
                    editor = HostConnectionPathEdit(existing: nil)
                }
            )
            Spacer(minLength: 0)
            WisentActionButton(
                action: WisentAction(
                    "Probe again",
                    symbol: "arrow.clockwise",
                    isEnabled: !linkStore.isRefreshing && !store.mutation.isWorking
                ) {
                    Task { await refresh() }
                }
            )
            WisentActionButton(
                action: WisentAction("Done", kind: .primary) {
                    dismiss()
                }
            )
        }
    }

    private func removalDialog(_ path: HostConnectionPathProbe) -> WisentDecisionDialog {
        let arguments = HostConnectionPathStore.removeArguments(host: host, name: path.name)
        return WisentDecisionDialog(
            tone: .danger,
            title: "Remove \(path.name) from \(host)?",
            lines: [
                "Stado will stop trying \(path.destination) when every route before it is unavailable.",
            ],
            listing: [StadoCLI.commandLine(arguments)],
            footnote: "The preferred primary route cannot be removed; it can only be replaced.",
            actions: [
                WisentAction("Keep route", kind: .secondary) { pendingRemoval = nil },
                WisentAction("Remove route", symbol: "trash", kind: .primary) {
                    pendingRemoval = nil
                    Task {
                        if await store.remove(host: host, name: path.name) {
                            await refresh()
                        }
                    }
                },
            ]
        )
    }
}
