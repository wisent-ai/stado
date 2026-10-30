import Foundation
import SwiftUI
import WisentDesignSystem

/// What holds the selected Stado API host's disk, ranked as the next cleanup
/// candidates: `stado space candidates --json`, run by that host's Stado.
/// It only reads. Agent output comes first, then caches, then the owner's
/// own large items, which are proposals only.
struct CandidatesSection: View {
    @ObservedObject var fleetStore: FleetControlStore
    @State private var report: CandidatesReport?
    @State private var problem: String?
    @State private var working = false

    var body: some View {
        Section("Cleanup candidates on the API host") {
            Text("Everything that holds the API host's disk, largest first within each class: agent output builds make again, then caches tools fetch again, then the owner's own files and folders. The janitor's pass at the 80% rule takes the automatic rows; every proposal is the owner's to decide, and nothing in Stado deletes user data.")
                .font(WisentTypeScale.caption())
            Button("Rank cleanup candidates") {
                Task { await load() }
            }
            .disabled(working || !fleetStore.isConfigured)
            if working { ProgressView("Measuring the home…") }
            if let problem { Text(problem).foregroundStyle(WisentDesign.danger).textSelection(.enabled) }
            if let report {
                Text(report.summary).font(WisentTypeScale.caption())
                ForEach(report.rows) { row in
                    LabeledContent(row.path, value: "\(row.size) · \(row.className) · \(row.action) · \(row.safety) · \(row.producer)")
                        .textSelection(.enabled)
                }
                if !report.removed.isEmpty {
                    DisclosureGroup("What the janitor removed") {
                        ForEach(report.removed, id: \.self) { line in
                            Text(line).font(WisentTypeScale.identifier()).textSelection(.enabled)
                        }
                    }
                }
            }
        }
    }

    @MainActor
    private func load() async {
        guard !working, let address = fleetStore.address else { return }
        let generation = fleetStore.requestGeneration
        working = true
        problem = nil
        defer { working = false }
        do {
            let result = try await fleetStore.client.run(
                arguments: ["space", "candidates", "--json"],
                confirmsMutation: false, at: address,
                authorizationToken: fleetStore.authorizationToken
            )
            guard generation == fleetStore.requestGeneration else { return }
            report = nil
            guard result.ok else {
                problem = result.message
                return
            }
            report = try CandidatesReport(json: result.standardOutput)
        } catch {
            guard generation == fleetStore.requestGeneration else { return }
            problem = error.localizedDescription
        }
    }
}

/// What `stado space candidates --json` printed.
private struct CandidatesReport {
    let rows: [CandidateRow]
    let removed: [String]
    let summary: String

    init(json: String) throws {
        guard let document = try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any] else {
            throw CandidatesReportError(detail: "the answer is not a JSON object")
        }
        guard let rows = document["candidates"] as? [[String: Any]] else {
            throw CandidatesReportError(detail: "the answer has no candidates array")
        }
        guard let need = document["need_bytes"] as? NSNumber,
              let rule = document["rule"] as? [String: Any],
              let used = rule["used_percent"] as? NSNumber,
              let threshold = rule["full_percent"] as? NSNumber
        else {
            throw CandidatesReportError(detail: "the answer has no need_bytes or rule")
        }
        self.rows = try rows.map(CandidateRow.init)
        removed = (document["removed"] as? [[String: Any]] ?? []).map { line in
            let what = (line["path"] as? String) ?? "\((line["items"] as? NSNumber)?.intValue ?? 0) item(s)"
            let bytes = ByteCountFormatter.string(fromByteCount: (line["bytes"] as? NSNumber)?.int64Value ?? 0, countStyle: .file)
            return "\(line["removed_at"] as? String ?? "") \(line["cleaner"] as? String ?? "") \(bytes) \(what)"
        }
        let needText = ByteCountFormatter.string(fromByteCount: need.int64Value, countStyle: .file)
        summary = need.int64Value > 0
            ? "\(String(format: "%.1f", used.doubleValue))% used; \(needText) to free before the volume is under the \(threshold.intValue)% rule"
            : "\(String(format: "%.1f", used.doubleValue))% used; under the \(threshold.intValue)% rule"
    }
}

private struct CandidateRow: Identifiable {
    let path: String
    let size: String
    let className: String
    let action: String
    let safety: String
    let producer: String

    var id: String { path }

    init(_ row: [String: Any]) throws {
        guard let path = row["path"] as? String,
              let bytes = row["bytes"] as? NSNumber,
              let className = row["class"] as? String,
              let action = row["action"] as? String,
              let safety = row["safety"] as? String,
              let producer = row["producer"] as? String
        else {
            throw CandidatesReportError(detail: "a row lacks path, bytes, class, action, safety or producer")
        }
        self.path = path
        size = ByteCountFormatter.string(fromByteCount: bytes.int64Value, countStyle: .file)
        self.className = className
        self.action = action
        self.safety = safety
        self.producer = producer
    }
}

private struct CandidatesReportError: LocalizedError {
    let detail: String
    var errorDescription: String? { "stado space candidates --json printed an unreadable answer: \(detail)" }
}
