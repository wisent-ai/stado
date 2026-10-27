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
        return "\(StadoFormat.decimal(free)) free of \(StadoFormat.decimal(total)) GiB"
    }

    /// A host publishes `free_vram_gb` null when its admission is closed and
    /// nothing read the cards, so the card total stands alone then.
    func vram(_ host: WorkerNode) -> String {
        guard let total = host.totalVRAMGB, !total.isZero, total.sign == .plus else {
            return "Not reported"
        }
        guard let free = host.freeVRAMGB, free.sign == .plus else {
            return "\(StadoFormat.decimal(total)) GiB, free not measured"
        }
        return "\(StadoFormat.decimal(free)) free of \(StadoFormat.decimal(total)) GiB"
    }
}
