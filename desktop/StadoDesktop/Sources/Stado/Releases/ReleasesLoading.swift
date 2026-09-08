import SwiftUI
import WisentDesignSystem

/// The reads: the inventory pass, and the per-row evidence that follows a
/// selection.
///
/// `reload`, `loadEvidence` and `reloadLogs` are internal rather than private
/// only because `body` sits in `ReleasesView.swift`, `placeholder` in
/// `ReleasesChrome.swift` and the stream pickers in
/// `Detail/ReleasesCandidate.swift`: Swift scopes `private` to one file.
extension ReleasesView {
    // MARK: Loading

    func reload() async {
        await store.refresh()
        let pair = selection.flatMap { current in
            store.rows.first { $0.pair == current }?.pair
        } ?? store.rows.first?.pair
        guard let pair else { return }
        if selection == pair {
            await loadEvidence(for: pair)
        } else {
            // Selecting drives the evidence read through onChange, so the
            // logs and the quarantine map are never read twice for one row.
            selection = pair
        }
    }

    func loadEvidence(for pair: ReleaseInventoryPair) async {
        await store.loadLogs(for: pair, stream: stream, lines: lines)
        await store.loadQuarantine(for: pair)
    }

    func reloadLogs(_ pair: ReleaseInventoryPair) {
        Task { await store.loadLogs(for: pair, stream: stream, lines: lines) }
    }
}
