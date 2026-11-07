import Combine
import SwiftUI
import WisentDesignSystem

/// `stado host processes TARGET [--within SECONDS] --json`: what each process
/// spent since the reading before, and which grow fast enough to leave the
/// host no memory before the next reading.
struct HostProcesses: Decodable, Sendable {
    struct Load: Decodable, Identifiable, Sendable {
        let pid: Int32
        let program: String
        let startedInInterval: Bool
        let cpuSeconds: Double
        let residentBytes: UInt64
        /// Absent from a host whose installed Stado predates the field; the
        /// row then says it is not reported, as the CLI's column does.
        let residentChangeBytes: Int64?
        let diskWrittenBytes: UInt64?

        var id: Int32 { pid }

        enum CodingKeys: String, CodingKey {
            case pid, program
            case startedInInterval = "started_in_interval"
            case cpuSeconds = "cpu_seconds"
            case residentBytes = "resident_bytes"
            case residentChangeBytes = "resident_change_bytes"
            case diskWrittenBytes = "disk_written_bytes"
        }
    }

    struct Unreadable: Decodable, Sendable {
        let pid: Int32
        let error: String
    }

    let state: String
    let notCompared: String?
    let intervalSeconds: Double?
    let processes: [Load]
    let read: Int
    let unreadable: [Unreadable]
    let memory: HostMemory?

    enum CodingKeys: String, CodingKey {
        case state, processes, read, unreadable, memory
        case notCompared = "not_compared"
        case intervalSeconds = "interval_seconds"
    }

    /// The report's first line, worded as `stado host processes` prints it.
    var heading: String {
        guard state == "compared", let intervalSeconds else {
            return "first reading, \(read) process(es) read; the next run reports what each spent since this one"
        }
        return "over \(intervalSeconds.formatted()) s, \(processes.count) of \(read) process(es) spent CPU or moved bytes"
    }
}

/// The one place the Hosts screen runs `stado host processes` and holds what
/// it answered for one host.
@MainActor
final class HostProcessesStore: ObservableObject {
    @Published private(set) var report: HostProcesses?
    @Published private(set) var problem: String?
    @Published private(set) var isReading = false

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    /// The command's words; `within` is the seconds until the next reading
    /// the operator names, or none for the interval just measured.
    nonisolated static func arguments(host: String, within: UInt64?) -> [String] {
        var words = ["host", "processes", host, "--json"]
        if let within { words += ["--within", String(within)] }
        return words
    }

    func read(host: String, within: UInt64?) async {
        guard !isReading else { return }
        isReading = true
        defer { isReading = false }
        do {
            report = try await cli.json(HostProcesses.self, arguments: Self.arguments(host: host, within: within))
            problem = nil
        } catch {
            report = nil
            problem = error.localizedDescription
        }
    }
}

/// What each process on a host spent since the reading before and which
/// would run it out of memory first: the same answer as `stado host
/// processes TARGET [--within SECONDS]`, in the command's order.
struct HostProcessesSection: View {
    let host: String
    @StateObject private var store = HostProcessesStore()
    /// The seconds until the next reading the operator names; empty for the
    /// interval just measured.
    @State private var within = ""

    /// `within` as the command takes it: a whole number above zero, or none.
    private var seconds: UInt64? { UInt64(within.trimmingCharacters(in: .whitespaces)).flatMap { $0 > 0 ? $0 : nil } }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            HStack {
                Text("Processes").font(WisentTypeScale.section())
                Spacer()
                TextField("Next reading in seconds", text: $within).frame(maxWidth: 200)
                Button("Read") { Task { await store.read(host: host, within: seconds) } }
                    .disabled(store.isReading || (!within.isEmpty && seconds == nil))
            }
            Text(StadoCLI.commandLine(HostProcessesStore.arguments(host: host, within: seconds)))
                .font(WisentTypeScale.identifier())
                .textSelection(.enabled)
            if !within.isEmpty && seconds == nil {
                Text("--within takes a whole number of seconds above zero").foregroundStyle(WisentDesign.warning)
            }
            if let problem = store.problem {
                Text(problem).foregroundStyle(WisentDesign.danger).textSelection(.enabled)
            }
            if let report = store.report {
                Text(report.heading).font(WisentTypeScale.caption())
                if let notCompared = report.notCompared {
                    Text("not compared with the kept reading: \(notCompared)").foregroundStyle(WisentDesign.warning)
                }
                if report.state == "compared" {
                    WisentField(
                        label: "Memory",
                        value: HostMemory.description(of: report.memory),
                        tone: HostMemory.tone(of: report.memory)
                    )
                }
                ForEach(report.processes) { load in
                    Text(
                        "pid \(load.pid) · CPU \(load.cpuSeconds.formatted()) s · RSS \(load.residentBytes) B "
                            + "(\(Self.change(load.residentChangeBytes))) · \(load.program)"
                    )
                    .font(WisentTypeScale.identifier())
                    .textSelection(.enabled)
                }
                ForEach(report.unreadable, id: \.pid) { refused in
                    Text("unreadable: pid \(refused.pid): \(refused.error)").foregroundStyle(WisentDesign.warning)
                }
            }
        }
        .task(id: host) { await store.read(host: host, within: nil) }
    }

    /// A process's resident change, signed as the CLI prints it, or
    /// `not reported` from a host whose Stado does not state it.
    static func change(_ bytes: Int64?) -> String {
        guard let bytes else { return "not reported" }
        return bytes >= 0 ? "+\(bytes)" : "\(bytes)"
    }
}
