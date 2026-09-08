import SwiftUI
import WisentDesignSystem

extension HostsView {
    func label(for status: WorkerAvailability) -> String {
        switch status {
        case .live: "Live"
        case .stale: "Stale"
        case .unavailable: "Unavailable"
        }
    }

    func tone(for status: WorkerAvailability) -> WisentTone {
        switch status {
        case .live: .success
        case .stale: .warning
        case .unavailable: .danger
        }
    }

    func admission(_ host: WorkerNode) -> String {
        switch host.acceptingJobs {
        case true: "Accepting jobs"
        case false: "Busy or gated"
        case nil: "Not reported"
        }
    }

    func accelerators(_ host: WorkerNode) -> String {
        guard !host.availableAccelerators.isEmpty else {
            return "None currently available"
        }
        return host.availableAccelerators
            .sorted { $0.key < $1.key }
            .map { "\($0.key) \($0.value)" }
            .joined(separator: " · ")
    }

    func cpu(_ host: WorkerNode) -> String {
        guard let available = host.availableCPUCores, let total = host.totalCPUCores else {
            return "Not reported"
        }
        return "\(available.formatted(.number)) available of \(total.formatted(.number)) cores"
    }

    func ram(_ host: WorkerNode) -> String {
        guard let free = host.freeRAMGB, let total = host.totalRAMGB else {
            return "Not reported"
        }
        return "\(StadoFormat.decimal(free)) free of \(StadoFormat.decimal(total)) GB"
    }

    func vram(_ host: WorkerNode) -> String {
        guard let total = host.totalVRAMGB, total > 0, let free = host.freeVRAMGB, free >= 0 else {
            return "Not reported"
        }
        return "\(StadoFormat.decimal(free)) free of \(StadoFormat.decimal(total)) GB"
    }

    func thresholds(_ cleanup: FleetCleanupPolicy?) -> String {
        guard let cleanup, let low = cleanup.lowFreeGB, let target = cleanup.targetFreeGB else {
            return "Not declared"
        }
        return "low \(low) GB · target \(target) GB"
    }
}
