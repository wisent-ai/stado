import SwiftUI
import WisentDesignSystem

/// Reclamation, in two steps that cannot be collapsed into one.
///
/// `--dry-run` runs when the sheet opens and its stages are on screen before an
/// apply exists at all; the store refuses an apply for a host it holds no dry
/// run for, so there is no route from a button to a deletion nobody previewed.
/// The reason is typed rather than picked, because a fixed list of reasons is a
/// list of the reasons somebody imagined in advance, and the audit record is
/// read by a person months later. Both commands are printed exactly as they run.
struct HostReclaimSheet: View {
    @ObservedObject var store: HostGatesStore
    let host: String
    let gates: HostGates?
    let refreshGates: () async -> Void
    let dismiss: () -> Void

    @State var reason = ""

    var trimmedReason: String {
        reason.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Both readings are host-scoped on the way out as well as on the way in.
    /// The store holds one pass at a time, and a dry run left behind by another
    /// host must never be the thing an operator reads before applying to this
    /// one.
    var preview: HostReclaimPass? {
        store.preview.flatMap { $0.host == host ? $0 : nil }
    }

    private var applied: HostReclaimPass? {
        store.applied.flatMap { $0.host == host ? $0 : nil }
    }

    var canApply: Bool {
        store.hasPreview(for: host) && !trimmedReason.isEmpty && !store.mutation.isWorking
    }

    var previewCommand: String {
        StadoCLI.commandLine(HostGatesStore.previewArguments(host: host))
    }

    /// The exact argv, with the reason as typed. A placeholder stands in while
    /// the field is empty so the operator can see where their words will land.
    var applyCommand: String {
        StadoCLI.commandLine(
            HostGatesStore.applyArguments(
                host: host,
                reason: trimmedReason.isEmpty ? "why this host needs the space" : trimmedReason
            )
        )
    }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            header
            if let applied {
                appliedSection(applied)
            } else {
                previewSection
                reasonSection
            }
            WisentMutationBar(outcome: store.mutation, clear: { store.clearMutation() })
            footer
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 640)
        .background(WisentDesign.canvas)
        .task {
            guard applied == nil, !store.hasPreview(for: host) else { return }
            await store.loadPreview(host: host)
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("Reclaim disk on \(host)")
                .font(WisentTypography.heading(17))
                .foregroundStyle(WisentDesign.ink)
            Text("Reclamation deletes what the registry's cleanup policy declares deletable on this host. It is why a host at 2 GB free against a 55 GB watermark starts claiming work again, and it is a deletion: it does not ask the host twice.")
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if let disk = gates?.disk {
                Text(verbatim: diskLine(disk))
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(disk.isBelowWatermark == true ? WisentTone.danger.color : WisentDesign.secondary)
                    .textSelection(.enabled)
            }
        }
    }

    private func diskLine(_ disk: HostGatesDisk) -> String {
        var text = "\(StadoFormat.decimal(disk.freeGB)) GB free"
        if let low = disk.lowWatermarkGB {
            text += " · watermark \(StadoFormat.decimal(low)) GB"
        }
        if let target = disk.targetFreeGB {
            text += " · target \(StadoFormat.decimal(target)) GB"
        }
        if let mode = disk.policyMode {
            text += " · policy \(mode)"
        }
        return text
    }
}
