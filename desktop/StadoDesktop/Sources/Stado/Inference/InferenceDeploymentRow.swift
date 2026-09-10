import SwiftUI
import WisentDesignSystem

/// One declared deployment: the model coordinate an operator approved, the
/// host and engine that serve it, and what the host's beacon last said.
struct InferenceDeploymentRow: View {
    let deployment: InferenceDeployment
    let beacon: InferenceBeacon?
    let beaconProblem: String?

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            HStack(alignment: .center, spacing: WisentDesign.Space.x3) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                    Text(deployment.name)
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.ink)
                        .textSelection(.enabled)
                    Text(deployment.model.coordinate)
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.secondary)
                        .textSelection(.enabled)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                WisentBadge(stateLabel, tone: stateTone)
            }
            HStack(spacing: WisentDesign.Space.x4) {
                Text(deployment.target)
                Text("\(deployment.engine.name)")
                Text("\(deployment.resources.gpus) GPU \(deployment.resources.gpuMode)")
                Text("context \(deployment.resources.maxModelLen.formatted(.number))")
                if let kvCache = deployment.resources.kvCacheMemoryGb {
                    Text("KV cache \(kvCache.formatted(.number)) GB")
                } else {
                    Text("KV cache unbounded")
                }
                Text("\(deployment.endpoint.host):\(String(deployment.endpoint.port)) \(deployment.endpoint.visibility)")
                if let used = beacon?.gpuMemoryUsedMb {
                    Text("GPU memory used \(used) MB")
                }
            }
            .font(WisentTypeScale.caption())
            .foregroundStyle(WisentDesign.secondary)
            if let beaconProblem {
                Text("Beacon not read: \(beaconProblem)")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.warning)
                    .textSelection(.enabled)
            } else if let detail = beacon?.detail {
                Text(detail)
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.muted)
                    .textSelection(.enabled)
            }
        }
        .padding(.vertical, WisentDesign.Space.x3)
    }

    /// The beacon's word when it has one; the registry's desired state,
    /// marked as such, when the beacon could not be read.
    private var stateLabel: String {
        if let beacon {
            return beacon.state
        }
        return "declared \(deployment.desiredState)"
    }

    private var stateTone: WisentTone {
        guard let beacon else { return .neutral }
        switch beacon.state {
        case "running", "active": return .success
        case "missing", "unknown": return .warning
        default: return .danger
        }
    }
}
