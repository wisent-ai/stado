import SwiftUI
import WisentDesignSystem

/// Release state, verbatim from `stado release status`: desired vs observed
/// per product target, then the newest pipeline runs with their persisted
/// failures. The dashboard's operator console serves the same text, so this
/// screen, the web console, and the CLI read one source and cannot drift.
struct ReleasesView: View {
    @ObservedObject var fleetStore: FleetControlStore
    let scope: String

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            header
            content
        }
        .padding(WisentDesign.Space.x6)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .task { await fleetStore.refreshReleaseStatus() }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: WisentDesign.Space.x3) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Releases")
                    .font(WisentTypeScale.screenTitle())
                    .foregroundStyle(WisentDesign.ink)
                Text("Desired vs observed per product, then the newest pipeline runs and why any of them failed — \(scope)")
                    .font(WisentTypography.body(12))
                    .foregroundStyle(WisentDesign.secondary)
            }
            Spacer(minLength: 0)
            if let updated = fleetStore.releaseStatusUpdated {
                Text(updated.formatted(date: .omitted, time: .standard))
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
            }
            Button {
                Task { await fleetStore.refreshReleaseStatus() }
            } label: {
                Label("Refresh", systemImage: "arrow.clockwise")
            }
            .disabled(fleetStore.isLoadingReleaseStatus)
        }
    }

    @ViewBuilder
    private var content: some View {
        if let problem = fleetStore.releaseStatusError {
            Text(problem)
                .font(WisentTypography.body(12))
                .foregroundStyle(WisentTone.danger.color)
                .textSelection(.enabled)
        }
        if let output = fleetStore.releaseStatusOutput, !output.isEmpty {
            ScrollView {
                Text(output)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(WisentDesign.ink)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .topLeading)
                    .padding(WisentDesign.Space.x4)
            }
            .background(WisentDesign.surface)
            .clipShape(RoundedRectangle(cornerRadius: WisentDesign.Radius.small))
            .overlay {
                RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
                    .stroke(WisentDesign.border, lineWidth: WisentDesign.hairline)
            }
        } else if fleetStore.isLoadingReleaseStatus {
            Text("Reading release state…")
                .font(WisentTypography.body(12))
                .foregroundStyle(WisentDesign.muted)
        } else if fleetStore.releaseStatusError == nil {
            Text("No release state has been read yet.")
                .font(WisentTypography.body(12))
                .foregroundStyle(WisentDesign.muted)
        }
    }
}
