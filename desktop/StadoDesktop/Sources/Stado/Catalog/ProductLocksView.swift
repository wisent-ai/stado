import SwiftUI
import WisentDesignSystem

struct ProductLocksView: View {
    let locks: [ProductLock]
    let failure: String?

    var body: some View {
        WisentSectionBox(title: "Installation locks",
                         detail: "Observed lock holder, process birth and last installation step on this host.") {
            if let failure {
                WisentErrorBanner(title: "Lock inspection refused", detail: failure)
            } else if locks.isEmpty {
                Text("No product surface lock records on this host")
                    .foregroundStyle(WisentDesign.secondary)
            } else {
                ForEach(locks) { lock in
                    VStack(alignment: .leading) {
                        Text("\(lock.product)/\(lock.surface): \(state(lock))")
                            .font(WisentTypeScale.identifierSmall())
                        Text(lock.path).textSelection(.enabled)
                        if let error = lock.error {
                            Text(error).textSelection(.enabled)
                        } else {
                            if let pid = lock.pid { Text("PID \(pid)") }
                            if let started = lock.startedAt { Text("Born \(started)") }
                            if let observed = lock.observedStart { Text("Observed \(observed)") }
                            if let phase = lock.phase { Text("Phase \(phase)") }
                            if let step = lock.step { Text("Step \(step)") }
                            if let acquired = lock.acquiredAt { Text("Acquired \(acquired)") }
                        }
                    }
                    .textSelection(.enabled)
                }
            }
        }
    }

    private func state(_ lock: ProductLock) -> String {
        if lock.error != nil { return "unreadable or corrupt" }
        if lock.held == true && lock.running == true { return "held" }
        if lock.held == true { return "held with unverified process identity" }
        if lock.running == true { return "released record" }
        return "stale record"
    }
}
