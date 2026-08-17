import SwiftUI
import WisentDesignSystem

struct WorkersView: View {
    let snapshot: DashboardSnapshot
    @State private var searchText = ""

    private var workers: [WorkerNode] {
        filter(snapshot.workers)
    }

    private var unavailableWorkers: [WorkerNode] {
        workers.filter { $0.status == .unavailable }
    }

    private var staleWorkers: [WorkerNode] {
        workers.filter { $0.status == .stale }
    }

    private var liveWorkers: [WorkerNode] {
        workers.filter { $0.status == .live }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(WisentDesign.Space.x6)

            if snapshot.workers.isEmpty {
                ContentUnavailableView {
                    Label("No registered workers", systemImage: "server.rack")
                } description: {
                    Text("The Stado registry and capacity store did not expose any workers in the current dashboard snapshot.")
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if workers.isEmpty {
                ContentUnavailableView.search(text: searchText)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List {
                    if !unavailableWorkers.isEmpty {
                        Section("Unavailable · \(unavailableWorkers.count)") {
                            ForEach(unavailableWorkers) { worker in
                                WorkerRow(worker: worker)
                            }
                        }
                    }
                    if !staleWorkers.isEmpty {
                        Section("Stale · \(staleWorkers.count)") {
                            ForEach(staleWorkers) { worker in
                                WorkerRow(worker: worker)
                            }
                        }
                    }
                    if !liveWorkers.isEmpty {
                        Section("Live · \(liveWorkers.count)") {
                            ForEach(liveWorkers) { worker in
                                WorkerRow(worker: worker)
                            }
                        }
                    }
                }
                .listStyle(.inset)
            }
        }
        .searchable(text: $searchText, prompt: "Search workers, hosts, GPUs, or reasons")
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text("Workers and nodes")
                    .font(.largeTitle.weight(.semibold))
                Text("Registered compute targets reconciled with capacity reports")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            WisentBadge(headerStatus.label, tone: headerStatus.tone)
        }
    }

    private var headerStatus: (label: String, tone: WisentTone) {
        let unavailable = snapshot.workers.count { $0.status == .unavailable }
        if unavailable > 0 {
            return ("\(unavailable) unavailable", .danger)
        }
        let stale = snapshot.workers.count { $0.status == .stale }
        if stale > 0 {
            return ("\(stale) stale", .warning)
        }
        let live = snapshot.workers.count { $0.status == .live }
        return ("\(live) live", live == 0 ? .warning : .success)
    }

    private func filter(_ workers: [WorkerNode]) -> [WorkerNode] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return workers }
        return workers.filter { worker in
            [
                worker.targetName,
                worker.consumerID,
                worker.kind,
                worker.gpuType,
                worker.role,
                worker.availabilityReason,
            ]
            .compactMap { $0 }
            .contains { $0.localizedCaseInsensitiveContains(query) }
            || worker.hostnames.contains { $0.localizedCaseInsensitiveContains(query) }
            || worker.freeSlots.keys.contains { $0.localizedCaseInsensitiveContains(query) }
        }
    }
}

private struct WorkerRow: View {
    let worker: WorkerNode

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                    HStack(spacing: WisentDesign.Space.x2) {
                        Text(worker.displayName)
                            .font(.headline)
                            .textSelection(.enabled)
                        WisentBadge(statusLabel, tone: statusTone)
                    }
                    Text(workerDescription)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                VStack(alignment: .trailing, spacing: WisentDesign.Space.x1) {
                    Text(capacityLabel)
                        .font(.subheadline.weight(.semibold))
                        .monospacedDigit()
                    Text(ageLabel)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }

            if worker.status != .live || !worker.declared {
                Label(worker.availabilityReason, systemImage: statusSymbol)
                    .font(.caption)
                    .foregroundStyle(statusTone.color)
                    .fixedSize(horizontal: false, vertical: true)
            }

            if !worker.hostnames.isEmpty {
                Text("Hosts: \(worker.hostnames.joined(separator: ", "))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }

            if worker.status != .unavailable, !worker.freeSlots.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: WisentDesign.Space.x2) {
                        ForEach(worker.freeSlots.keys.sorted(), id: \.self) { slotType in
                            Text("\(slotType): \(worker.freeSlots[slotType, default: 0])")
                                .font(.caption.monospacedDigit())
                                .padding(.horizontal, WisentDesign.Space.x2)
                                .padding(.vertical, WisentDesign.Space.x1)
                                .background(.quaternary, in: Capsule())
                        }
                    }
                }
                .accessibilityLabel("Free slot breakdown")
            }

            if let total = worker.totalVRAMGB,
               let free = worker.freeVRAMGB,
               total > 0,
               free >= 0 {
                let used = max(0, total - min(free, total))
                HStack(spacing: WisentDesign.Space.x3) {
                    ProgressView(value: used, total: total)
                        .frame(width: StadoLayout.progressWidth)
                    Text("\(StadoFormat.decimal(used)) of \(StadoFormat.decimal(total)) GB VRAM allocated")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .accessibilityElement(children: .combine)
            }
        }
        .padding(.vertical, WisentDesign.Space.x2)
        .accessibilityElement(children: .contain)
    }

    private var statusLabel: String {
        switch worker.status {
        case .live: "Live"
        case .stale: "Stale"
        case .unavailable: "Unavailable"
        }
    }

    private var statusTone: WisentTone {
        switch worker.status {
        case .live: worker.declared ? .success : .warning
        case .stale: .warning
        case .unavailable: .danger
        }
    }

    private var statusSymbol: String {
        switch worker.status {
        case .live: "exclamationmark.triangle"
        case .stale: "clock.badge.exclamationmark"
        case .unavailable: "xmark.circle"
        }
    }

    private var workerDescription: String {
        let hardware = worker.gpuType?.humanizedIdentifier
            ?? worker.kind?.humanizedIdentifier
            ?? "Worker kind unavailable"
        if let role = worker.role?.humanizedIdentifier {
            return "\(hardware) · \(role)"
        }
        return hardware
    }

    private var capacityLabel: String {
        worker.status == .unavailable ? "No capacity report" : "\(worker.availableSlots) free slots"
    }

    private var ageLabel: String {
        if let age = worker.ageSeconds {
            return "Reported \(StadoFormat.duration(age)) ago"
        }
        if let date = StadoFormat.date(worker.publishedAt) {
            return "Reported \(date.formatted(.relative(presentation: .named)))"
        }
        return "Never reported"
    }
}

