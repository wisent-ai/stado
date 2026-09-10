import SwiftUI
import WisentDesignSystem

extension HostsView {
    // MARK: Gates

    var gateHostNames: [String] {
        StadoRegistryHosts.names(targets: fleetStore.targets, snapshot: store.snapshot)
    }

    func hostGates(_ host: WorkerNode) -> HostGates? {
        gatesStore.gates(for: host.targetName ?? host.displayName)
    }

    func gateFailure(_ host: WorkerNode) -> String? {
        gatesStore.failure(for: host.targetName ?? host.displayName)
    }

    func claimingLabel(_ host: WorkerNode) -> String {
        guard let gates = hostGates(host) else {
            return gateFailure(host) == nil ? "Not read" : "Unreadable"
        }
        guard gates.complete == true, let claiming = gates.claiming else { return "Unknown" }
        if claiming { return "Yes" }
        return gates.pinnedByDesign && gates.waitingJobs.isEmpty ? "Pinned" : "No"
    }

    func claimingTone(_ host: WorkerNode) -> WisentTone {
        guard let gates = hostGates(host) else { return .warning }
        guard gates.complete == true, let claiming = gates.claiming else { return .warning }
        if claiming { return .success }
        // The declared pin is neutral until it starves a job addressed to this
        // host; every other refusal is a failure.
        return gates.pinnedByDesign && gates.waitingJobs.isEmpty ? .neutral : .danger
    }

    func gateReason(_ host: WorkerNode) -> String {
        if let failure = gateFailure(host) {
            return failure
        }
        guard let gates = hostGates(host) else {
            if gatesStore.isRefreshing { return "Reading its gates…" }
            return host.declared
                ? "stado host gates has not answered for this host"
                : "Not a declared registry target"
        }
        guard gates.complete == true, let claiming = gates.claiming else {
            return "Diagnostic reads are incomplete; inspect their source and failure."
        }
        if claiming { return "" }
        if gates.pinnedByDesign {
            return gates.waitingJobs.isEmpty
                ? "Pinned by the registry: claims only work addressed to this host"
                : "Pinned by the registry, and pinned work is waiting"
        }
        return gates.blockers.isEmpty
            ? "Claiming nothing, and the host named no blocker"
            : gates.blockers.joined(separator: " · ")
    }

    func freeDisk(_ host: WorkerNode) -> String {
        guard let disk = hostGates(host)?.disk, let free = disk.freeGB else {
            return "Not reported"
        }
        guard let low = disk.lowWatermarkGB else {
            return "\(StadoFormat.decimal(free)) GB"
        }
        return "\(StadoFormat.decimal(free)) of \(StadoFormat.decimal(low)) GB"
    }

    func diskTone(_ host: WorkerNode) -> WisentTone {
        hostGates(host)?.disk?.isBelowWatermark == true ? .danger : .neutral
    }

    func cpuCell(_ host: WorkerNode) -> String {
        if let available = hostGates(host)?.capacity?.availableCPUCores {
            return available.formatted(.number)
        }
        return host.availableCPUCores?.formatted(.number) ?? "—"
    }

    /// The capacity report's own age when the host published one, and the
    /// dashboard's otherwise. Both are the same clock; the host's is closer to
    /// the machine.
    func reportAge(_ host: WorkerNode) -> Double? {
        hostGates(host)?.capacity?.ageSeconds ?? host.ageSeconds
    }

    func diskDescription(_ disk: HostGatesDisk?) -> String {
        guard let disk, let free = disk.freeGB else { return "Not reported" }
        var text = "\(StadoFormat.decimal(free)) GB free"
        if let low = disk.lowWatermarkGB {
            text += " · claims stop below \(StadoFormat.decimal(low)) GB"
        }
        if let target = disk.targetFreeGB {
            text += " · cleanup aims for \(StadoFormat.decimal(target)) GB"
        }
        return text
    }

    func capacityDescription(_ capacity: HostGatesCapacity?) -> String {
        guard let capacity else { return "Not reported" }
        let admission = switch capacity.acceptingJobs {
        case true: "Accepting jobs"
        case false: "Busy or gated"
        case nil: "Admission not reported"
        }
        let running = capacity.runningJobs.map { "\($0.formatted(.number)) running" } ?? "running unknown"
        let cpu: String
        if let available = capacity.availableCPUCores, let total = capacity.totalCPUCores {
            cpu = "\(available.formatted(.number))/\(total.formatted(.number)) CPU cores available"
        } else {
            cpu = "CPU not reported"
        }
        let ram: String
        if let free = capacity.freeRAMGB, let total = capacity.totalRAMGB {
            ram = "\(StadoFormat.decimal(free))/\(StadoFormat.decimal(total)) GB RAM free"
        } else {
            ram = "RAM not reported"
        }
        return "\(admission) · \(running) · \(cpu) · \(ram)"
    }
}
