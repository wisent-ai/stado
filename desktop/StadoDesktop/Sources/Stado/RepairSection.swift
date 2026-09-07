import Foundation
import SwiftUI
import WisentDesignSystem

private struct RepairCatalog: Decodable, Sendable {
    let declaration: String
    let services: [RepairService]
}

struct RepairService: Decodable, Identifiable, Sendable {
    let name: String
    let summary: String
    let repair: [RepairDeclaration]

    var id: String { name }
}

struct RepairDeclaration: Decodable, Identifiable, Sendable {
    let name: String
    let summary: String
    let mutating: Bool
    let proof: String

    var id: String { name }
}

struct RepairReport: Decodable, Sendable {
    let declaration: String
    let service: String
    let target: String?
    let applied: Bool
    let steps: [RepairStepReport]
}

struct RepairStepReport: Decodable, Identifiable, Sendable {
    let name: String
    let summary: String
    let mutating: Bool
    let proof: String
    let status: String
    let observation: RepairObservation

    var id: String { name }
}

indirect enum RepairObservation: Decodable, Sendable {
    case object([String: RepairObservation])
    case array([RepairObservation])
    case string(String)
    case integer(Int64)
    case number(Double)
    case boolean(Bool)
    case null

    init(from decoder: Decoder) throws {
        if let container = try? decoder.container(keyedBy: RepairJSONKey.self) {
            var fields: [String: RepairObservation] = [:]
            for key in container.allKeys {
                fields[key.stringValue] = try container.decode(RepairObservation.self, forKey: key)
            }
            self = .object(fields)
            return
        }
        if var container = try? decoder.unkeyedContainer() {
            var values: [RepairObservation] = []
            while !container.isAtEnd {
                values.append(try container.decode(RepairObservation.self))
            }
            self = .array(values)
            return
        }
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .boolean(value)
        } else if let value = try? container.decode(Int64.self) {
            self = .integer(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else {
            self = .string(try container.decode(String.self))
        }
    }

    var text: String {
        switch self {
        case let .object(fields):
            fields.keys.sorted().map { key in
                "\(key): \(fields[key]?.text ?? "null")"
            }.joined(separator: "\n")
        case let .array(values):
            values.map(\.text).joined(separator: "\n")
        case let .string(value): value
        case let .integer(value): String(value)
        case let .number(value): value.formatted(.number.precision(.fractionLength(0 ... 3)))
        case let .boolean(value): value ? "true" : "false"
        case .null: "null"
        }
    }
}

private struct RepairJSONKey: CodingKey {
    let stringValue: String
    let intValue: Int?

    init?(stringValue: String) {
        self.stringValue = stringValue
        intValue = nil
    }

    init?(intValue: Int) {
        stringValue = String(intValue)
        self.intValue = intValue
    }
}

@MainActor
final class RepairStore: ObservableObject {
    @Published private(set) var services: [RepairService] = []
    @Published private(set) var declaration = "stado-rs/data/service-catalog.json"
    @Published private(set) var reports: [String: RepairReport] = [:]
    @Published private(set) var loadingCatalog = false
    @Published private(set) var running: String?
    @Published private(set) var problem: String?
    @Published private(set) var mutation: WisentMutationOutcome = .idle

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func listArguments() -> [String] {
        ["repair", "list", "--json"]
    }

    nonisolated static func runArguments(
        service: String,
        host: String,
        step: String? = nil,
        apply: Bool
    ) -> [String] {
        var arguments = ["repair", service]
        if let step {
            arguments.append(contentsOf: ["--step", step])
        }
        arguments.append(contentsOf: ["--target", host])
        if apply {
            arguments.append("--apply")
        }
        arguments.append("--json")
        return arguments
    }

    func load() async {
        guard services.isEmpty, !loadingCatalog else { return }
        loadingCatalog = true
        defer { loadingCatalog = false }
        do {
            let catalog = try await cli.json(
                RepairCatalog.self,
                arguments: Self.listArguments()
            )
            declaration = catalog.declaration
            services = catalog.services.filter { !$0.repair.isEmpty }
            problem = nil
        } catch {
            problem = Self.message(error)
        }
    }

    func report(service: String, host: String) -> RepairReport? {
        reports[Self.key(service: service, host: host)]
    }

    func isRunning(service: String, host: String) -> Bool {
        running == Self.key(service: service, host: host)
    }

    func run(service: String, host: String, step: String? = nil, apply: Bool) async {
        guard running == nil else { return }
        let key = Self.key(service: service, host: host)
        running = key
        mutation = .working(
            apply ? "Applying \(service)'s declared repair on \(host)" : "Reading \(service)'s repair plan on \(host)"
        )
        defer { running = nil }
        do {
            let report = try await cli.json(
                RepairReport.self,
                arguments: Self.runArguments(
                    service: service,
                    host: host,
                    step: step,
                    apply: apply
                ),
                timeoutSeconds: apply ? nil : 180
            )
            reports[key] = report
            problem = nil
            mutation = .succeeded(
                apply
                    ? "Applied \(report.steps.count) declared repair step(s) on \(host)."
                    : "Read \(report.steps.count) declared repair step(s) on \(host) without applying them."
            )
        } catch {
            let message = Self.message(error)
            problem = message
            mutation = .failed(message)
        }
    }

    func clearMutation() {
        mutation = .idle
    }

    private nonisolated static func key(service: String, host: String) -> String {
        "\(host)|\(service)"
    }

    private nonisolated static func message(_ error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? String(describing: error)
    }
}

struct RepairSection: View {
    @ObservedObject var store: RepairStore
    let host: String

    @State private var pendingApply: RepairService?

    var body: some View {
        WisentSectionBox(
            title: "Declared repair",
            detail: "Every step, order, mutation boundary, and proof comes from \(store.declaration). A dry run reads the host without applying a step."
        ) {
            if store.loadingCatalog && store.services.isEmpty {
                WisentLoadingPanel(
                    title: "Reading repair declarations",
                    detail: "Loading the service catalog compiled into this Stado release."
                )
            } else if store.services.isEmpty, let problem = store.problem {
                WisentAlertPanel(
                    tone: .warning,
                    title: "Repair declarations could not be read",
                    detail: problem
                )
            } else {
                ForEach(store.services) { service in
                    serviceSection(service)
                }
            }
            WisentMutationBar(outcome: store.mutation) {
                store.clearMutation()
            }
        }
        .task {
            await store.load()
        }
        .alert(item: $pendingApply) { service in
            Alert(
                title: Text("Apply \(service.name) repair on \(host)?"),
                message: Text("Stado will run \(service.repair.count) mutating step(s) in declaration order and read each declared proof afterwards."),
                primaryButton: .destructive(Text("Apply")) {
                    Task {
                        await store.run(service: service.name, host: host, apply: true)
                    }
                },
                secondaryButton: .cancel()
            )
        }
    }

    @ViewBuilder
    private func serviceSection(_ service: RepairService) -> some View {
        let report = store.report(service: service.name, host: host)
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            Text(service.name)
                .font(WisentTypeScale.bodyStrong())
                .foregroundStyle(WisentDesign.ink)
            Text(service.summary)
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
            ForEach(service.repair) { step in
                WisentField(
                    label: step.name,
                    value: "\(step.summary)\nProof: \(step.proof)",
                    tone: step.mutating ? .warning : .neutral
                )
            }
            HStack(spacing: WisentDesign.Space.x2) {
                WisentActionButton(
                    action: WisentAction(
                        "Dry run",
                        symbol: "doc.text.magnifyingglass",
                        kind: .secondary,
                        isEnabled: !store.isRunning(service: service.name, host: host)
                    ) {
                        Task {
                            await store.run(service: service.name, host: host, apply: false)
                        }
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Apply declared steps…",
                        symbol: "wrench.and.screwdriver",
                        kind: .primary,
                        isEnabled: !store.isRunning(service: service.name, host: host)
                    ) {
                        pendingApply = service
                    }
                )
            }
            if let report {
                WisentField(
                    label: "Last report",
                    value: report.applied ? "Applied on \(report.target ?? host)" : "Dry run on \(report.target ?? host)"
                )
                ForEach(report.steps) { step in
                    WisentField(
                        label: "\(step.name) · \(step.status)",
                        value: "\(step.observation.text)\nProof read: \(step.proof)",
                        tone: step.status == "applied" ? .success : .neutral
                    )
                }
            }
        }
        .padding(.vertical, WisentDesign.Space.x2)
    }
}
