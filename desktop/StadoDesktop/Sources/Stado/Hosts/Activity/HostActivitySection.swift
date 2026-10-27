import Combine
import SwiftUI
import WisentDesignSystem

/// `stado host activity TARGET --json`: every job a host's worker ran.
struct HostActivity: Decodable, Equatable, Sendable {
    struct Entry: Decodable, Equatable, Identifiable, Sendable {
        let at: String?
        let job: String
        let lifecycle: String
        let state: String
        let command: String
        let kind: String?
        let seconds: Int?
        let error: String?
        let unreadable: [String]

        var id: String { "\(lifecycle)/\(job)" }

        /// The entry's heading; an entry whose record states no readable
        /// time says that instead of a time.
        var heading: String {
            guard let at else { return "\(lifecycle) · \(job) · its record states no readable time" }
            return "\(at) · \(lifecycle) · \(job)"
        }
    }

    let host: String
    let jobs: [Entry]
    let seconds: Int
}

/// The one place the Hosts screen runs `stado host activity` and holds what
/// it answered for one host.
@MainActor
final class HostActivityStore: ObservableObject {
    @Published private(set) var activity: HostActivity?
    @Published private(set) var problem: String?
    @Published private(set) var isReading = false

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func arguments(host: String) -> [String] {
        ["host", "activity", host, "--json"]
    }

    func read(host: String) async {
        guard !isReading else { return }
        isReading = true
        defer { isReading = false }
        do {
            activity = try await cli.json(HostActivity.self, arguments: Self.arguments(host: host))
            problem = nil
        } catch {
            activity = nil
            problem = error.localizedDescription
        }
    }
}

/// What a host ran, under what the host is now. The same answer as
/// `stado host activity TARGET`, newest last, as the command orders it.
struct HostActivitySection: View {
    let host: String
    @StateObject private var store = HostActivityStore()

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            HStack {
                Text("Activity").font(WisentTypeScale.section())
                Spacer()
                Button("Read") { Task { await store.read(host: host) } }
                    .disabled(store.isReading)
            }
            Text(StadoCLI.commandLine(HostActivityStore.arguments(host: host)))
                .font(WisentTypeScale.identifier())
                .textSelection(.enabled)
            if let problem = store.problem {
                Text(problem).foregroundStyle(WisentDesign.danger).textSelection(.enabled)
            }
            if let activity = store.activity {
                if activity.jobs.isEmpty {
                    Text("No job of the queue names this host as its worker.")
                        .font(WisentTypeScale.caption())
                }
                ForEach(activity.jobs) { entry in
                    VStack(alignment: .leading) {
                        Text(entry.heading).font(WisentTypeScale.identifier())
                        Text(entry.command).font(WisentTypeScale.caption()).textSelection(.enabled)
                        if let error = entry.error {
                            Text(error).foregroundStyle(WisentDesign.danger).textSelection(.enabled)
                        }
                        ForEach(entry.unreadable, id: \.self) { problem in
                            Text(problem).foregroundStyle(WisentDesign.warning)
                        }
                    }
                }
                Text("\(activity.jobs.count) job(s), \(activity.seconds) measured second(s)")
                    .font(WisentTypeScale.caption())
            }
        }
        .task(id: host) { await store.read(host: host) }
    }
}
