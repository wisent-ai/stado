import SwiftUI
import WisentDesignSystem

/// One composed patch, waiting for the operator to read it.
struct MemoryReviewRequest: Identifiable {
    let target: String
    let patch: MemoryReclaimPatch

    /// The reviewed body itself: a different patch is a different sheet.
    var id: String { patch.canonicalJSON(target: target) }
}

/// The review step every registry write on this console passes through: what
/// each field does when the host reads it, the route, and the exact body.
struct MemoryReviewDialog: View {
    let request: MemoryReviewRequest
    let generation: String?
    let cancel: () -> Void
    let confirm: () -> Void

    var body: some View {
        WisentDecisionDialog(
            tone: request.patch.authorizesRepairs ? .danger : .warning,
            title: "Write memory_reclaim on \(request.target)?",
            lines: request.patch.reviewLines + [
                "The write is a compare-and-swap on the canonical registry. If the fleet's registry moved since generation \(generation ?? "unknown") was read, the dashboard refuses the write and nothing changes.",
                "Arming or disarming an individual repair is not part of this write. The repairs stay exactly as the registry declares them.",
            ],
            reasonCode: request.patch.authorizesRepairs
                ? "enforce authorizes declared repairs on this host"
                : "declaration only",
            listing: [
                "POST /api/registry/policy",
                request.patch.canonicalJSON(target: request.target),
            ],
            footnote: "Registry generation \(generation ?? "unknown") at the time this screen was read.",
            actions: [
                WisentAction("Leave the declaration unchanged", kind: .secondary) { cancel() },
                WisentAction(
                    request.patch.authorizesRepairs ? "Authorize repairs" : "Write the declaration",
                    kind: request.patch.authorizesRepairs ? .destructive : .primary
                ) { confirm() },
            ]
        )
    }
}
