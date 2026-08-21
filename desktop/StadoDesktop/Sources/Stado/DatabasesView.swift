import SwiftUI
import WisentDesignSystem

/// The fleet's database plane: what is declared, where each database is
/// placed, and who may resolve it.
///
/// Read-only on purpose. Resolving a database hands an endpoint and a
/// credential coordinate to the consumer that asks; this screen shows the
/// declarations and their placement so an operator can see the plane without
/// becoming one of its consumers.
struct DatabasesView: View {
    @ObservedObject var store: DatabasesStore
    let scope: String

    var body: some View {
        WisentScreen(
            title: "Databases",
            scope: scope,
            freshness: store.lastUpdated.map { "Read \(ConsoleFormat.relative($0))" },
            actions: [
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task { await store.refresh() }
                },
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: 0) {
                if let problem = store.problem {
                    WisentErrorBanner(title: "The database list did not answer", detail: problem)
                }
                if store.rows.isEmpty {
                    WisentEmptyPanel(
                        title: store.isRefreshing ? "Reading" : "No databases declared",
                        detail: store.isRefreshing
                            ? "stado database list --json against the canonical registry. Nothing is written."
                            : "Declare one under database_api.databases in the Stado configuration, then place it as a service.",
                        symbol: "cylinder"
                    )
                    Spacer(minLength: 0)
                } else {
                    table
                }
            }
        }
        .task { await store.refresh() }
    }

    private var table: some View {
        VStack(spacing: 0) {
            HStack(spacing: WisentDesign.Space.x3) {
                Text("DATABASE").frame(width: 140, alignment: .leading)
                Text("ENGINE").frame(width: 80, alignment: .leading)
                Text("PLACEMENT").frame(width: 180, alignment: .leading)
                Text("SCOPES").frame(width: 120, alignment: .leading)
                Text("CREDENTIAL ITEM").frame(maxWidth: .infinity, alignment: .leading)
            }
            .font(WisentTypeScale.eyebrow())
            .tracking(0.6)
            .foregroundStyle(WisentDesign.muted)
            .padding(.horizontal, WisentDesign.Space.x4)
            .padding(.vertical, WisentDesign.Space.x2)

            ForEach(store.rows) { row in
                HStack(spacing: WisentDesign.Space.x3) {
                    Text(row.database)
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.ink)
                        .frame(width: 140, alignment: .leading)
                    Text(row.engine)
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.muted)
                        .frame(width: 80, alignment: .leading)
                    Text(placement(row))
                        .font(WisentTypeScale.body())
                        .foregroundStyle(row.placed ? WisentDesign.success : WisentDesign.warning)
                        .frame(width: 180, alignment: .leading)
                    Text(row.scopes.joined(separator: ", "))
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.ink)
                        .frame(width: 120, alignment: .leading)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(row.item)
                            .font(WisentTypeScale.identifier())
                            .foregroundStyle(WisentDesign.ink)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Text("\(row.consumers.count) consumer\(row.consumers.count == 1 ? "" : "s")")
                            .font(WisentTypeScale.caption())
                            .foregroundStyle(WisentDesign.muted)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .padding(.horizontal, WisentDesign.Space.x4)
                .frame(height: WisentAppLayout.denseRowHeight)
            }
            Spacer(minLength: 0)
        }
    }

    private func placement(_ row: DatabaseRow) -> String {
        row.placed ? (row.activeHost.map { "placed · \($0)" } ?? "placed") : "not placed"
    }
}
