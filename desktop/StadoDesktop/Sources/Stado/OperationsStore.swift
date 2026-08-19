import Combine
import Foundation
import WisentDesignSystem

enum DashboardEndpointPreference {
    static let key = "dashboardBaseURL"
    /// The address this app adopted on its own. Storing it distinguishes "the
    /// operator typed this" from "we defaulted to this once", which is the
    /// difference between a setting and a leftover.
    static let chosenKey = "dashboardBaseURLAdopted"
    /// Last resort only. `127.0.0.1:8765` is this machine's own host-health
    /// API, which answers from the local copy of the store: on an operator
    /// laptop that copy is days behind, so the app showed "no capacity report
    /// exists" for hosts that were publishing every minute, and a blocked queue
    /// where the fleet had none. The fleet's address is the one every other
    /// reader already uses, so read it from the same file instead of keeping a
    /// fourth port written down somewhere new.
    static let fallbackURL = "http://127.0.0.1:8765"
    static let configuredKeyPath = ["storage", "stado", "url"]

    static var localURL: String {
        fleetURLFromConfig() ?? fallbackURL
    }

    /// `~/.config/stado/config.json` -> `storage.stado.url`, the canonical
    /// object API as this host reaches it (a resolver adapter on a laptop, the
    /// service itself on the authority host).
    static func fleetURLFromConfig(
        _ path: URL = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/stado/config.json")
    ) -> String? {
        guard let data = try? Data(contentsOf: path),
              let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        var node: Any? = root
        for key in configuredKeyPath {
            node = (node as? [String: Any])?[key]
        }
        guard let address = node as? String,
              !address.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { return nil }
        return address
    }

    static func load(from defaults: UserDefaults) -> String {
        let stored = defaults.string(forKey: key)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return stored.isEmpty ? localURL : stored
    }

    static func save(_ value: String, to defaults: UserDefaults) {
        defaults.set(value, forKey: key)
    }
}

@MainActor
final class OperationsStore: ObservableObject {
    @Published private(set) var snapshot: DashboardSnapshot?
    @Published private(set) var isRefreshing = false
    @Published private(set) var errorMessage: String?
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var dashboardURLString: String

    private let defaults: UserDefaults
    private let client: OperationsClient
    private var requestGeneration = 0
    private var authorizationToken: String?

    init(defaults: UserDefaults = .standard, client: OperationsClient = OperationsClient()) {
        self.defaults = defaults
        self.client = client
        dashboardURLString = DashboardEndpointPreference.load(from: defaults)
        adoptFleetAddressIfUnchosen()
    }

    /// Follow the fleet address without anybody retyping it.
    ///
    /// The address was stored once, years of restarts ago, and then pinned:
    /// when the fleet's store moved, this app kept reading the old one and
    /// showed every worker as unavailable until a human noticed and edited a
    /// setting. A value the operator never chose is not a choice, so a stored
    /// address that is merely a previous default gives way to what
    /// `~/.config/stado/config.json` names today. An address the operator typed
    /// is left alone -- that one IS a choice.
    func adoptFleetAddressIfUnchosen() {
        guard let fleet = DashboardEndpointPreference.fleetURLFromConfig() else { return }
        let chosen = defaults.string(forKey: DashboardEndpointPreference.chosenKey)
        let current = dashboardURLString.trimmingCharacters(in: .whitespacesAndNewlines)
        let inherited = current.isEmpty
            || current == DashboardEndpointPreference.fallbackURL
            || (chosen != nil && chosen != current)
        guard inherited, current != fleet else { return }
        dashboardURLString = fleet
        DashboardEndpointPreference.save(fleet, to: defaults)
        defaults.set(fleet, forKey: DashboardEndpointPreference.chosenKey)
        requestGeneration &+= 1
    }

    var dashboardAddress: OperationsDashboardAddress? {
        try? OperationsDashboardAddress(dashboardURLString)
    }

    var isConfigured: Bool {
        dashboardAddress != nil
    }

    var isShowingStaleSnapshot: Bool {
        snapshot != nil && errorMessage != nil
    }

    func configureAuthorization(token: String?) {
        authorizationToken = token
    }

    func refresh() async {
        guard !isRefreshing else { return }
        // Configuration can move while the app is open, and an operator should
        // not have to relaunch a viewer to see the fleet it points at. Adopt
        // before reading the address, or this tick would still use the old one.
        adoptFleetAddressIfUnchosen()
        guard let address = dashboardAddress else {
            errorMessage = nil
            return
        }
        let generation = requestGeneration
        isRefreshing = true
        defer {
            if requestGeneration == generation {
                isRefreshing = false
            }
        }

        do {
            let newSnapshot = try await client.fetchState(
                from: address,
                authorizationToken: authorizationToken
            )
            guard requestGeneration == generation, !Task.isCancelled else { return }
            snapshot = newSnapshot
            lastUpdated = Date()
            errorMessage = nil
        } catch is CancellationError {
            return
        } catch let error as URLError where error.code == .cancelled {
            return
        } catch {
            guard requestGeneration == generation else { return }
            errorMessage = Self.displayMessage(for: error)
        }
    }

    func testDashboardURL(_ value: String) async throws -> String {
        let address = try OperationsDashboardAddress(value)
        _ = try await client.fetchState(from: address, authorizationToken: authorizationToken)
        return address.displayString
    }

    func clearDashboardURL() {
        requestGeneration &+= 1
        dashboardURLString = ""
        snapshot = nil
        lastUpdated = nil
        errorMessage = nil
        isRefreshing = false
    }

    func saveDashboardURL(_ value: String) throws {
        let address = try OperationsDashboardAddress(value)
        requestGeneration &+= 1
        dashboardURLString = address.displayString
        DashboardEndpointPreference.save(address.displayString, to: defaults)
        snapshot = nil
        lastUpdated = nil
        errorMessage = nil
        isRefreshing = false
        Task { await refresh() }
    }

    private static func displayMessage(for error: Error) -> String {
        if let urlError = error as? URLError {
            switch urlError.code {
            case .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed, .networkConnectionLost, .notConnectedToInternet, .timedOut:
                return "The Stado dashboard could not be reached. Start the local dashboard or update the endpoint in Settings."
            default:
                return "The Stado dashboard request failed."
            }
        }
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return "The Stado dashboard request failed."
    }
}

// MARK: - Host gates and managed services

/// Which hosts the two CLI-backed screens ask.
///
/// `stado host gates` and `stado service converge` both take a declared
/// registry target, so the canonical projection is the list. When that
/// projection has not been read, the snapshot's declared targets stand in
/// rather than nothing being asked at all: a host whose gates went unread is
/// precisely the host these screens exist for.
enum StadoRegistryHosts {
    static func names(targets: [FleetPolicyTarget], snapshot: DashboardSnapshot?) -> [String] {
        if !targets.isEmpty {
            return targets.map(\.name).sorted()
        }
        let declared = (snapshot?.workers ?? []).filter(\.declared).compactMap(\.targetName)
        return Array(Set(declared)).sorted()
    }
}

/// What one host answered when it was asked whether it is claiming work, and
/// the two-step reclamation that is the only write on the Hosts screen.
///
/// Every value here comes from `stado host gates` and `stado host reclaim` run
/// as child processes: the same commands, the same words, the same exit codes
/// an operator would get in a terminal. Nothing on this screen is computed
/// from a second source.
@MainActor
final class HostGatesStore: ObservableObject {
    @Published private(set) var gates: [HostGates] = []
    /// Host name -> the command's own sentence, for a host whose gates could
    /// not be read. One unreachable host must not blank the other nine rows,
    /// and a row missing without explanation is how a silent host stays
    /// silent.
    @Published private(set) var failures: [String: String] = [:]
    @Published private(set) var isRefreshing = false
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var mutation: WisentMutationOutcome = .idle
    /// The dry run the operator has actually seen, bound to the host it was
    /// read for. `apply` refuses without it, so there is no path from a button
    /// to a deletion that skipped the preview.
    @Published private(set) var preview: HostReclaimPass?
    @Published private(set) var isPreviewing = false
    @Published private(set) var applied: HostReclaimPass?

    private let cli: StadoCLI
    private var refreshGeneration = 0

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    /// The alarm on the Hosts screen. A host that is not claiming work is the
    /// state that stalled every release build for hours while the fleet looked
    /// healthy from every other angle.
    var notClaiming: [HostGates] {
        gates.filter { !$0.claiming }
    }

    func gates(for host: String) -> HostGates? {
        gates.first { $0.host == host }
    }

    func failure(for host: String) -> String? {
        failures[host]
    }

    /// True only when the operator is looking at a dry run for this exact
    /// host. The apply button reads this, and so does `apply` itself.
    func hasPreview(for host: String) -> Bool {
        preview.map { $0.host == host && $0.isDryRun } ?? false
    }

    nonisolated static func gatesArguments(host: String) -> [String] {
        ["host", "gates", host, "--json"]
    }

    nonisolated static func previewArguments(host: String) -> [String] {
        ["host", "reclaim", host, "--dry-run", "--json"]
    }

    nonisolated static func applyArguments(host: String, reason: String) -> [String] {
        ["host", "reclaim", host, "--apply", "--reason", reason, "--json"]
    }

    /// Read-only. One `host gates` invocation per registry host, concurrently,
    /// because a fleet of twelve hosts read one after another takes longer than
    /// an operator will wait before reaching for a terminal.
    func refresh(hosts: [String]) async {
        guard !isRefreshing else { return }
        refreshGeneration += 1
        let generation = refreshGeneration
        isRefreshing = true
        defer {
            if generation == refreshGeneration {
                isRefreshing = false
            }
        }

        let reads = await Self.read(hosts: hosts, using: cli)
        guard generation == refreshGeneration else { return }
        gates = reads.compactMap(\.gates).sorted { lhs, rhs in
            lhs.claiming == rhs.claiming ? lhs.host < rhs.host : !lhs.claiming
        }
        failures = reads.reduce(into: [:]) { table, read in
            if let problem = read.problem { table[read.host] = problem }
        }
        lastUpdated = Date()
    }

    /// `--dry-run` first, always. The preview is what makes the apply legible:
    /// which stages would run, how much each one frees, and how many items it
    /// would touch.
    func loadPreview(host: String) async {
        guard !isPreviewing else { return }
        isPreviewing = true
        defer { isPreviewing = false }
        preview = nil
        applied = nil
        mutation = .working("Reading what reclamation would free on \(host)")
        do {
            let pass = try await cli.json(
                HostReclaimPass.self,
                arguments: Self.previewArguments(host: host)
            )
            preview = pass
            mutation = .idle
        } catch {
            mutation = .failed(Self.message(for: error))
        }
    }

    /// The write. It refuses without a dry run for this host and without a
    /// reason, in the store rather than only in the view: a screen is one
    /// caller, and the rule belongs where every caller meets it.
    func apply(host: String, reason: String) async {
        let reason = reason.trimmingCharacters(in: .whitespacesAndNewlines)
        guard hasPreview(for: host) else {
            mutation = .failed(
                "Nothing has been previewed for \(host). Run the dry run first and read what it would free."
            )
            return
        }
        guard !reason.isEmpty else {
            mutation = .failed("A reason is required: it is what the audit record will say months from now.")
            return
        }
        mutation = .working("Reclaiming disk on \(host)")
        do {
            let pass = try await cli.json(
                HostReclaimPass.self,
                arguments: Self.applyArguments(host: host, reason: reason)
            )
            applied = pass
            // The dry run described a host that no longer exists in that
            // state, so a second apply needs a second preview.
            preview = nil
            mutation = .succeeded(Self.summary(of: pass))
        } catch {
            mutation = .failed(Self.message(for: error))
        }
    }

    func clearReclamation() {
        preview = nil
        applied = nil
        mutation = .idle
    }

    func clearMutation() {
        mutation = .idle
    }

    private struct HostGatesRead: Sendable {
        let host: String
        var gates: HostGates?
        var problem: String?
    }

    private nonisolated static func read(hosts: [String], using cli: StadoCLI) async -> [HostGatesRead] {
        await withTaskGroup(of: HostGatesRead.self) { group in
            for host in hosts {
                group.addTask {
                    do {
                        return HostGatesRead(
                            host: host,
                            gates: try await cli.json(
                                HostGates.self,
                                arguments: gatesArguments(host: host)
                            )
                        )
                    } catch {
                        return HostGatesRead(host: host, problem: message(for: error))
                    }
                }
            }
            var reads: [HostGatesRead] = []
            reads.reserveCapacity(hosts.count)
            for await read in group {
                reads.append(read)
            }
            return reads
        }
    }

    private nonisolated static func summary(of pass: HostReclaimPass) -> String {
        let stages = pass.stages.count == 1 ? "1 stage" : "\(pass.stages.count) stages"
        guard let before = pass.freeGBBefore, let after = pass.freeGBAfter else {
            return "Reclamation ran \(stages) on \(pass.host); the command reported no free-space figures."
        }
        return "Reclamation ran \(stages) on \(pass.host): "
            + "\(StadoFormat.decimal(before)) GB free before, \(StadoFormat.decimal(after)) GB after."
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}

/// Every registry-managed service with the state its host's latest health
/// beacon reports, plus the one write an operator is allowed from here:
/// restarting a user-domain unit.
///
/// The read is one fleet-wide `service list --json` — beacon-only, so it
/// stays answerable while a host is wedged — followed by one `service status
/// --json` per failed service name, because the list does not carry failure
/// evidence and a red word with no reason behind it sends the operator to a
/// terminal. When the fleet-wide read itself fails, every host that was
/// asked contributes an unavailable row: one broken read must not blank the
/// screen, and it must not read as a fleet with nothing declared.
@MainActor
final class FleetServicesStore: ObservableObject {
    @Published private(set) var entries: [FleetServiceEntry] = []
    /// Host name -> the command's own sentence, set for every host that was
    /// asked when the fleet-wide read produced no answer at all.
    @Published private(set) var failures: [String: String] = [:]
    @Published private(set) var isRefreshing = false
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var mutation: WisentMutationOutcome = .idle

    private let cli: StadoCLI
    private var refreshGeneration = 0
    private var lastHosts: [String] = []

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    /// Failed units first, then by host and unit: the rows that need a
    /// decision read before the ones that do not.
    var services: [FleetServiceEntry] {
        entries.sorted { lhs, rhs in
            lhs.isFailed == rhs.isFailed ? lhs.id < rhs.id : lhs.isFailed
        }
    }

    var failedServices: [FleetServiceEntry] {
        entries.filter(\.isFailed)
    }

    /// The rows whose declared launchd domain their host cannot have, in the
    /// same order as `services` so a facet and the table agree.
    ///
    /// The finding rides on the `service list --json` row itself — the CLI
    /// checks the registry document against the target's own role — so no
    /// second read is needed and none is performed.
    var misdeclaredServices: [FleetServiceEntry] {
        services.filter { $0.misdeclaredDomain != nil }
    }

    nonisolated static func listArguments() -> [String] {
        ["service", "list", "--json"]
    }

    nonisolated static func statusArguments(name: String) -> [String] {
        ["service", "status", name, "--json"]
    }

    nonisolated static func restartArguments(name: String, host: String) -> [String] {
        ["service", "restart", name, "--host", host, "--json"]
    }

    nonisolated static func removeFileArguments(host: String, path: String) -> [String] {
        ["host", "remove-file", host, path, "--json"]
    }

    func refresh(hosts: [String]) async {
        guard !isRefreshing else { return }
        refreshGeneration += 1
        let generation = refreshGeneration
        lastHosts = hosts
        isRefreshing = true
        defer {
            if generation == refreshGeneration {
                isRefreshing = false
            }
        }

        let reading = await Self.read(using: cli)
        guard generation == refreshGeneration else { return }
        switch reading {
        case let .listed(listed):
            let failedNames = Array(Set(listed.filter(\.isFailed).map(\.name))).sorted()
            let evidence = await Self.failureEvidence(for: failedNames, using: cli)
            entries = listed.map { entry in
                var entry = entry
                entry.failure = evidence[entry.id]
                return entry
            }
            failures = [:]
        case let .failed(problem):
            entries = []
            failures = Dictionary(uniqueKeysWithValues: hosts.map { ($0, problem) })
        }
        lastUpdated = Date()
    }

    /// `stado service restart <name> --host <host>` through the CLI runner.
    ///
    /// The CLI refuses a system LaunchDaemon with its own sentence; the view
    /// never shows this button for one, and the refusal is still what a
    /// failure reports if the declaration moved under the screen. A restart
    /// that exits zero but left the host outside the intended state is read
    /// off the payload's postcondition, in the same words the CLI prints.
    func restart(_ entry: FleetServiceEntry) async {
        let unit = entry.unitID.isEmpty ? entry.name : entry.unitID
        mutation = .working("Restarting \(unit) on \(entry.host)")
        do {
            let reports = try await cli.json(
                [ServiceRestartReport].self,
                arguments: Self.restartArguments(name: entry.name, host: entry.host)
            )
            if let failed = reports.first(where: { !$0.succeeded }) {
                mutation = .failed("\(failed.host): \(failed.failureText)")
            } else {
                mutation = .succeeded("Restarted \(unit) on \(entry.host).")
            }
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        // Whatever happened, the beacon's next word is the one worth reading:
        // a succeeded restart shows as active, a refused one as the state it
        // was refused in.
        await refresh(hosts: lastHosts)
    }

    /// `stado host remove-file <host> <path>` through the CLI runner: the
    /// delete half `service retire` deliberately does not have. The CLI's
    /// guards decide on the host — the view offers this only where they could
    /// pass, and a refusal still arrives in the command's own sentence.
    func removeUnitFile(_ entry: FleetServiceEntry) async {
        mutation = .working("Removing \(entry.path) on \(entry.host)")
        do {
            let report = try await cli.json(
                RemoveFileReport.self,
                arguments: Self.removeFileArguments(host: entry.host, path: entry.path)
            )
            if report.succeeded {
                mutation = .succeeded(
                    report.status == "removed"
                        ? "Removed \(report.path) on \(entry.host)."
                        : "\(report.path) was already absent on \(entry.host)."
                )
            } else {
                mutation = .failed("\(report.target): \(report.status)\(report.detail.map { " — \($0)" } ?? "")")
            }
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        await refresh(hosts: lastHosts)
    }

    func clearMutation() {
        mutation = .idle
    }

    private enum ListReading: Sendable {
        case listed([FleetServiceEntry])
        case failed(String)
    }

    private nonisolated static func read(using cli: StadoCLI) async -> ListReading {
        do {
            return .listed(try await cli.json(FleetServiceList.self, arguments: listArguments()))
        } catch {
            return .failed(message(for: error))
        }
    }

    /// One `service status --json` per failed name, concurrently, keyed by
    /// the entry id the failure belongs to. A status read that fails costs
    /// that unit its evidence line, never the list it was annotating.
    private nonisolated static func failureEvidence(
        for names: [String],
        using cli: StadoCLI
    ) async -> [String: ServiceFailure] {
        guard !names.isEmpty else { return [:] }
        return await withTaskGroup(of: [String: ServiceFailure].self) { group in
            for name in names {
                group.addTask {
                    guard let rows = try? await cli.json(
                        FleetServiceList.self,
                        arguments: statusArguments(name: name)
                    ) else { return [:] }
                    var evidence: [String: ServiceFailure] = [:]
                    for row in rows where row.failure != nil {
                        evidence[row.id] = row.failure
                    }
                    return evidence
                }
            }
            var merged: [String: ServiceFailure] = [:]
            for await evidence in group {
                merged.merge(evidence) { _, new in new }
            }
            return merged
        }
    }

    /// `service list --json` prints a bare array; naming the type keeps the
    /// decode site reading like the command it runs.
    private typealias FleetServiceList = [FleetServiceEntry]

    /// Decode one `service list --json` payload without running anything, so
    /// the shape this console reads can be exercised against a recorded
    /// answer — including the registry finding three of the fleet's rows
    /// carry today.
    nonisolated static func decode(from output: String) -> [FleetServiceEntry]? {
        let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let data = trimmed.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(FleetServiceList.self, from: data)
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}

/// Why a host went quiet, per registry host.
///
/// The reading that did not exist during the six-minute gap on
/// `control-host`: `stado host link <host> --json` carries the beacon's
/// link block, the recorded silences, and how often a reader refused to answer
/// about the host while it was quiet. One invocation per host, concurrently,
/// for the same reason `host gates` is read that way — a fleet read serially is
/// a fleet the operator gives up on and opens a terminal for.
///
/// Read-only. This store performs no write, and nothing here is computed from a
/// second source: the verdict is the command's, and so are the blockers.
@MainActor
final class HostLinkStore: ObservableObject {
    @Published private(set) var links: [HostLink] = []
    /// Host name -> the command's own sentence, for a host whose link could not
    /// be read. A host that did not answer *about its own silence* is exactly
    /// the host this screen exists for, so the row says so rather than
    /// disappearing.
    @Published private(set) var failures: [String: String] = [:]
    @Published private(set) var isRefreshing = false
    @Published private(set) var lastUpdated: Date?

    private let cli: StadoCLI
    private var refreshGeneration = 0

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    /// Every host the command did not call healthy, worst first. The Posture
    /// and Hosts screens both aggregate from here rather than re-deriving it.
    var needingAttention: [HostLink] {
        links.filter { $0.verdict.needsAttention }
    }

    /// Hosts quiet right now: a silence record that has not closed.
    var silentNow: [HostLink] {
        links.filter { $0.openSilence != nil }
    }

    func link(for host: String) -> HostLink? {
        links.first { $0.host == host }
    }

    func failure(for host: String) -> String? {
        failures[host]
    }

    nonisolated static func linkArguments(host: String) -> [String] {
        ["host", "link", host, "--json"]
    }

    /// The command as an operator would type it, for every panel that quotes it.
    nonisolated static func commandLine(host: String) -> String {
        StadoCLI.commandLine(linkArguments(host: host))
    }

    func refresh(hosts: [String]) async {
        guard !isRefreshing else { return }
        refreshGeneration += 1
        let generation = refreshGeneration
        isRefreshing = true
        defer {
            if generation == refreshGeneration {
                isRefreshing = false
            }
        }

        let reads = await Self.read(hosts: hosts, using: cli)
        guard generation == refreshGeneration else { return }
        links = reads.compactMap(\.link).sorted { lhs, rhs in
            Self.attentionRank(lhs) == Self.attentionRank(rhs)
                ? lhs.host < rhs.host
                : Self.attentionRank(lhs) < Self.attentionRank(rhs)
        }
        failures = reads.reduce(into: [:]) { table, read in
            if let problem = read.problem { table[read.host] = problem }
        }
        lastUpdated = Date()
    }

    /// Silent before degraded before a verdict nobody recognised, healthy last.
    /// A host that is quiet right now outranks one that merely reported a
    /// degraded path.
    private nonisolated static func attentionRank(_ link: HostLink) -> Int {
        switch link.verdict {
        case .silent: 0
        case .degraded: 1
        case .unrecognised: 2
        case .healthy: 3
        }
    }

    /// Decode one `--json` payload without running anything, so the shape this
    /// console reads can be exercised against a recorded answer.
    nonisolated static func decode(from output: String) -> HostLink? {
        let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let data = trimmed.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(HostLink.self, from: data)
    }

    private struct HostLinkRead: Sendable {
        let host: String
        var link: HostLink?
        var problem: String?
    }

    private nonisolated static func read(hosts: [String], using cli: StadoCLI) async -> [HostLinkRead] {
        await withTaskGroup(of: HostLinkRead.self) { group in
            for host in hosts {
                group.addTask {
                    do {
                        return HostLinkRead(
                            host: host,
                            link: try await cli.json(
                                HostLink.self,
                                arguments: linkArguments(host: host)
                            )
                        )
                    } catch {
                        return HostLinkRead(host: host, problem: message(for: error))
                    }
                }
            }
            var reads: [HostLinkRead] = []
            reads.reserveCapacity(hosts.count)
            for await read in group {
                reads.append(read)
            }
            return reads
        }
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}

/// What is actually running on the fleet, as opposed to what is declared.
///
/// Two readings, because two different things were invisible. `service
/// converge` in report mode says what each declared unit runs and whether the
/// process is executing the code that is on disk; `service list --unowned`
/// says which product processes belong to no unit at all. Both are read-only:
/// this store performs no write.
@MainActor
final class ServiceTruthStore: ObservableObject {
    @Published private(set) var reports: [ServiceConvergeReport] = []
    @Published private(set) var unownedProcesses: [UnownedProcess] = []
    /// Host name -> the command's own sentence for a host whose units could
    /// not be read.
    @Published private(set) var failures: [String: String] = [:]
    @Published private(set) var unownedProblem: String?
    @Published private(set) var isRefreshing = false
    @Published private(set) var lastUpdated: Date?

    private let cli: StadoCLI
    private var refreshGeneration = 0

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    /// Declared units, with the host carried on the row and the units serving
    /// replaced code first.
    var units: [ServiceUnitRow] {
        reports
            .flatMap { report in
                report.units.map { ServiceUnitRow(host: report.target, unit: $0) }
            }
            .sorted { lhs, rhs in
                lhs.unit.servesReplacedCode == rhs.unit.servesReplacedCode
                    ? lhs.id < rhs.id
                    : lhs.unit.servesReplacedCode
            }
    }

    var mismatched: [ServiceUnitRow] {
        units.filter(\.unit.servesReplacedCode)
    }

    /// What a sidebar badge is allowed to count: a process serving code that is
    /// no longer on disk, and a process nothing owns.
    var attentionCount: Int {
        mismatched.count + unownedProcesses.count
    }

    func failure(for host: String) -> String? {
        failures[host]
    }

    nonisolated static func convergeArguments(host: String) -> [String] {
        ["service", "converge", host, "--json"]
    }

    nonisolated static func unownedArguments() -> [String] {
        ["service", "list", "--unowned", "--json"]
    }

    func refresh(hosts: [String]) async {
        guard !isRefreshing else { return }
        refreshGeneration += 1
        let generation = refreshGeneration
        isRefreshing = true
        defer {
            if generation == refreshGeneration {
                isRefreshing = false
            }
        }

        let readings = await Self.read(hosts: hosts, using: cli)
        guard generation == refreshGeneration else { return }
        reports = readings.reports.sorted { $0.target < $1.target }
        failures = readings.failures
        unownedProcesses = readings.unowned.sorted { lhs, rhs in
            lhs.host == rhs.host ? lhs.pid < rhs.pid : lhs.host < rhs.host
        }
        unownedProblem = readings.unownedProblem
        lastUpdated = Date()
    }

    private struct Readings: Sendable {
        var reports: [ServiceConvergeReport] = []
        var failures: [String: String] = [:]
        var unowned: [UnownedProcess] = []
        var unownedProblem: String?
    }

    private enum Reading: Sendable {
        case converged(ServiceConvergeReport)
        case convergeFailed(host: String, problem: String)
        case unowned([UnownedProcess])
        case unownedFailed(String)
    }

    private nonisolated static func read(hosts: [String], using cli: StadoCLI) async -> Readings {
        await withTaskGroup(of: Reading.self) { group in
            for host in hosts {
                group.addTask {
                    do {
                        return .converged(
                            try await cli.json(
                                ServiceConvergeReport.self,
                                arguments: convergeArguments(host: host)
                            )
                        )
                    } catch {
                        return .convergeFailed(host: host, problem: message(for: error))
                    }
                }
            }
            group.addTask {
                do {
                    let report = try await cli.json(
                        UnownedProcessReport.self,
                        arguments: unownedArguments()
                    )
                    return .unowned(report.processes)
                } catch {
                    return .unownedFailed(message(for: error))
                }
            }

            var readings = Readings()
            for await reading in group {
                switch reading {
                case let .converged(report):
                    readings.reports.append(report)
                case let .convergeFailed(host, problem):
                    readings.failures[host] = problem
                case let .unowned(processes):
                    readings.unowned = processes
                case let .unownedFailed(problem):
                    readings.unownedProblem = problem
                }
            }
            return readings
        }
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}

// MARK: - Release evidence

/// What each product should be running, what its host is actually running,
/// and the two things that stop a rollout dead: a quarantined digest nobody
/// can clear from a screen, and a candidate that died with its reason in a
/// file on the host.
///
/// The inventory of product/target pairs is one cheap read; every diagnosis
/// after it reaches a host, so they are issued concurrently and each row is
/// published the moment its own host answers. One unreachable host leaves one
/// row carrying the command's sentence, never a blank table.
@MainActor
final class ReleaseEvidenceStore: ObservableObject {
    @Published private(set) var rows: [ReleaseRow] = []
    /// The inventory read itself failed: there is no list of rollouts to
    /// diagnose, which is a different state from "every rollout is fine".
    @Published private(set) var inventoryProblem: String?
    @Published private(set) var isRefreshing = false
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var mutation: WisentMutationOutcome = .idle
    /// What each host reported it runs, as the last inventory read stated it.
    /// Held apart from `rows` so re-diagnosing one rollout cannot drop the
    /// software finding that rollout had nothing to do with.
    private var softwareReports: [ReleaseInventoryPair: ReleaseSoftwareReport] = [:]

    /// Logs are read for the pair the operator is looking at, never for the
    /// whole fleet: each tail is a read off a host.
    @Published private(set) var logs: ReleaseLogsReport?
    @Published private(set) var logsProblem: String?
    @Published private(set) var isLoadingLogs = false
    @Published private(set) var logsPair: ReleaseInventoryPair?

    @Published private(set) var quarantine: ReleaseQuarantineReport?
    @Published private(set) var quarantineProblem: String?
    @Published private(set) var isLoadingQuarantine = false
    @Published private(set) var quarantinePair: ReleaseInventoryPair?
    /// The audit record the last clearance wrote, kept on screen so the
    /// operator can name the backup file without going to the host.
    @Published private(set) var clearance: ReleaseQuarantineClearance?
    /// The newest pipeline runs, straight from the same inventory read. A run
    /// in flight shows where each platform stands; a failed one carries its
    /// recorded failure, so an operator learns why here, not in a terminal.
    @Published private(set) var pipelineRuns: [ReleasePipelineRunRecord] = []

    private let cli: StadoCLI
    private var refreshGeneration = 0
    private var logsGeneration = 0
    private var quarantineGeneration = 0

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    /// What a sidebar badge is allowed to count: a rollout the fleet itself
    /// calls blocked, and one this console could not diagnose at all.
    var attentionCount: Int {
        rows.count { row in
            switch row.diagnosis {
            case .pending: false
            case let .diagnosed(report): report.verdict == .blocked
            case .failed: true
            }
        }
    }

    func row(for pair: ReleaseInventoryPair?) -> ReleaseRow? {
        guard let pair else { return nil }
        return rows.first { $0.pair == pair }
    }

    nonisolated static func inventoryArguments() -> [String] {
        ["release", "status", "--json"]
    }

    nonisolated static func doctorArguments(pair: ReleaseInventoryPair) -> [String] {
        ["release", "doctor", pair.product, "--target", pair.target, "--json"]
    }

    nonisolated static func logsArguments(
        pair: ReleaseInventoryPair,
        stream: ReleaseLogStreamSelection,
        lines: Int
    ) -> [String] {
        [
            "release", "logs", pair.product,
            "--target", pair.target,
            "--stream", stream.rawValue,
            "--lines", String(lines),
            "--json",
        ]
    }

    nonisolated static func quarantineArguments(pair: ReleaseInventoryPair) -> [String] {
        ["release", "quarantine", "list", pair.product, "--target", pair.target, "--json"]
    }

    nonisolated static func clearArguments(
        pair: ReleaseInventoryPair,
        digest: String,
        reason: String
    ) -> [String] {
        [
            "release", "quarantine", "clear", pair.product,
            "--target", pair.target,
            "--digest", digest,
            "--reason", reason,
            "--json",
        ]
    }

    /// Read-only. The inventory first, then one `release doctor` per pair,
    /// concurrently, each row replaced as its host answers.
    func refresh() async {
        guard !isRefreshing else { return }
        refreshGeneration += 1
        let generation = refreshGeneration
        isRefreshing = true
        defer {
            if generation == refreshGeneration {
                isRefreshing = false
            }
        }

        // The pair and its software verdict travel together from here on. The
        // verdict is the CLI's, decided before this process saw it, so a
        // re-diagnosis of the rollout must not silently drop it — a row that
        // loses its software finding on refresh is a row that goes quiet again.
        var software: [ReleaseInventoryPair: ReleaseSoftwareReport] = [:]
        let pairs: [ReleaseInventoryPair]
        do {
            let inventory = try await cli.json(
                ReleaseInventory.self,
                arguments: Self.inventoryArguments()
            )
            guard generation == refreshGeneration else { return }
            inventoryProblem = nil
            let entries = inventory.entries
                .filter { !$0.pair.product.isEmpty && !$0.pair.target.isEmpty }
                .sorted { $0.pair.id < $1.pair.id }
            for entry in entries {
                if let report = entry.software {
                    software[entry.pair] = report
                }
            }
            pipelineRuns = inventory.runs
            pairs = entries.map(\.pair)
        } catch {
            guard generation == refreshGeneration else { return }
            // The rows already on screen stay: a refresh that failed does not
            // erase the last diagnosis the operator was reading.
            inventoryProblem = Self.message(for: error)
            return
        }

        softwareReports = software
        rows = pairs.map { ReleaseRow(pair: $0, diagnosis: .pending, software: software[$0]) }
        var diagnoses: [ReleaseInventoryPair: ReleaseDiagnosis] = [:]
        let cli = cli
        await withTaskGroup(of: (ReleaseInventoryPair, ReleaseDiagnosis).self) { group in
            for pair in pairs {
                group.addTask {
                    (pair, await Self.diagnosis(of: pair, using: cli))
                }
            }
            for await (pair, diagnosis) in group {
                guard generation == refreshGeneration else { continue }
                diagnoses[pair] = diagnosis
                rows = Self.ordered(
                    pairs.map {
                        ReleaseRow(
                            pair: $0,
                            diagnosis: diagnoses[$0] ?? .pending,
                            software: software[$0]
                        )
                    }
                )
            }
        }
        guard generation == refreshGeneration else { return }
        lastUpdated = Date()
    }

    /// One rollout, re-read. Used after a clearance, so the screen states the
    /// host's answer rather than the operator's expectation of it.
    ///
    /// `release doctor` says nothing about installed software, so the software
    /// verdict the inventory carried is kept as it was. Dropping it here would
    /// make one clearance quietly clear a finding nobody addressed.
    func diagnose(_ pair: ReleaseInventoryPair) async {
        let diagnosis = await Self.diagnosis(of: pair, using: cli)
        guard let index = rows.firstIndex(where: { $0.pair == pair }) else { return }
        rows[index] = ReleaseRow(
            pair: pair,
            diagnosis: diagnosis,
            software: softwareReports[pair]
        )
        rows = Self.ordered(rows)
        lastUpdated = Date()
    }

    /// The candidate's own stdout/stderr, off the host that ran it.
    func loadLogs(
        for pair: ReleaseInventoryPair,
        stream: ReleaseLogStreamSelection,
        lines: Int
    ) async {
        logsGeneration += 1
        let generation = logsGeneration
        if logsPair != pair {
            logs = nil
        }
        logsPair = pair
        logsProblem = nil
        isLoadingLogs = true
        defer {
            if generation == logsGeneration {
                isLoadingLogs = false
            }
        }
        do {
            let report = try await cli.json(
                ReleaseLogsReport.self,
                arguments: Self.logsArguments(pair: pair, stream: stream, lines: lines)
            )
            guard generation == logsGeneration else { return }
            logs = report
        } catch {
            guard generation == logsGeneration else { return }
            logs = nil
            logsProblem = Self.message(for: error)
        }
    }

    /// The digests this host refuses to roll out again, each told whether it
    /// is the digest the registry currently desires.
    func loadQuarantine(for pair: ReleaseInventoryPair) async {
        quarantineGeneration += 1
        let generation = quarantineGeneration
        if quarantinePair != pair {
            quarantine = nil
            clearance = nil
        }
        quarantinePair = pair
        quarantineProblem = nil
        isLoadingQuarantine = true
        defer {
            if generation == quarantineGeneration {
                isLoadingQuarantine = false
            }
        }
        do {
            let report = try await cli.json(
                ReleaseQuarantineReport.self,
                arguments: Self.quarantineArguments(pair: pair)
            )
            guard generation == quarantineGeneration else { return }
            quarantine = report
        } catch {
            guard generation == quarantineGeneration else { return }
            quarantine = nil
            quarantineProblem = Self.message(for: error)
        }
    }

    /// The one write this screen performs. It refuses without a reason here,
    /// in the store, rather than only in the dialog: the audit record is the
    /// point of the command, and a screen is only one of its callers.
    ///
    /// Nothing is started, stopped or restarted afterwards. The release agent
    /// picks the digest up on its next tick, and the store re-reads the host
    /// so the rollout's state on screen is the host's answer.
    func clearQuarantine(
        pair: ReleaseInventoryPair,
        digest: String,
        reason: String
    ) async {
        let reason = reason.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !reason.isEmpty else {
            mutation = .failed("A reason is required: it is what the audit record will say months from now.")
            return
        }
        guard !digest.isEmpty else {
            mutation = .failed("No digest was named, and this command clears exactly one.")
            return
        }
        mutation = .working("Clearing \(digest) for \(pair.product) on \(pair.target)")
        do {
            let result = try await cli.json(
                ReleaseQuarantineClearance.self,
                arguments: Self.clearArguments(pair: pair, digest: digest, reason: reason)
            )
            clearance = result
            mutation = .succeeded(Self.summary(of: result))
            await loadQuarantine(for: pair)
            await diagnose(pair)
        } catch {
            mutation = .failed(Self.message(for: error))
        }
    }

    func clearMutation() {
        mutation = .idle
    }

    private nonisolated static func diagnosis(
        of pair: ReleaseInventoryPair,
        using cli: StadoCLI
    ) async -> ReleaseDiagnosis {
        do {
            return .diagnosed(
                try await cli.json(
                    ReleaseDoctorReport.self,
                    arguments: doctorArguments(pair: pair)
                )
            )
        } catch {
            return .failed(message(for: error))
        }
    }

    /// Blocked rollouts first, then the ones nobody could diagnose, then the
    /// ones still moving. A settled rollout is the row an operator scrolls
    /// past, so it sits at the bottom.
    private nonisolated static func ordered(_ rows: [ReleaseRow]) -> [ReleaseRow] {
        rows.sorted { lhs, rhs in
            lhs.attentionRank == rhs.attentionRank
                ? lhs.id < rhs.id
                : lhs.attentionRank < rhs.attentionRank
        }
    }

    private nonisolated static func summary(of clearance: ReleaseQuarantineClearance) -> String {
        "Cleared \(clearance.digest) for \(clearance.product) on \(clearance.target). "
            + "Nothing was started, stopped or restarted; the release agent rolls this digest out on its next tick."
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}
