import Foundation
import WisentDesignSystem

/// The five `stado scratch` invocations, run as the product CLI and decoded
/// from their `--json` documents.
///
/// Nothing here composes a lease, an expiry or a TTL: the durations are the
/// declaration's own strings, the account states are read off the machine,
/// and a refusal is surfaced in the CLI's own sentence, because "profile
/// 'macos-account' is declared for platforms darwin-arm64, darwin-amd64" is a
/// sentence an operator can act on and "the command failed" is not.
@MainActor
final class ScratchStore: ObservableObject {
    @Published private(set) var catalog: ScratchProfileCatalog?
    @Published private(set) var listing: ScratchLeaseListing?
    @Published private(set) var lease: ScratchLeaseReceipt?
    @Published private(set) var destroyed: ScratchDestroyReceipt?
    @Published private(set) var reapPlan: ScratchReapReport?
    @Published private(set) var reapReceipt: ScratchReapReport?
    @Published private(set) var refusal: String?
    @Published private(set) var mutation: WisentMutationOutcome = .idle
    @Published private(set) var isReading = false

    /// What the create button will run. A value of its own so the argument
    /// vector and the TTL default are decided in one readable place.
    @Published var form = ScratchCreateForm()

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    var profiles: [ScratchProfile] { catalog?.profiles ?? [] }

    var selectedProfile: ScratchProfile? { profiles.first { $0.name == form.profile } }

    var leases: [ScratchLease] { listing?.leases ?? [] }

    var expiredLeases: [ScratchLease] { leases.filter(\.expired) }

    /// Choosing a profile seeds the TTL field with that profile's declared
    /// default, so the duration in the field is always one the declaration
    /// named.
    func selectProfile(named name: String) {
        guard let profile = profiles.first(where: { $0.name == name }) else { return }
        form.select(profile)
    }

    // MARK: The five invocations

    nonisolated static func profilesArguments() -> [String] {
        ["scratch", "profiles", "--json"]
    }

    nonisolated static func listArguments(host: String) -> [String] {
        ["scratch", "list", "--host", host, "--json"]
    }

    nonisolated static func createArguments(
        host: String,
        profile: String,
        name: String?,
        ttl: String?,
        root: String?
    ) -> [String] {
        var arguments = ["scratch", "create", "--host", host, "--profile", profile]
        if let name = present(name) { arguments.append(contentsOf: ["--name", name]) }
        if let ttl = present(ttl) { arguments.append(contentsOf: ["--ttl", ttl]) }
        if let root = present(root) { arguments.append(contentsOf: ["--root", root]) }
        arguments.append("--json")
        return arguments
    }

    nonisolated static func destroyArguments(name: String, host: String) -> [String] {
        ["scratch", "destroy", name, "--host", host, "--json"]
    }

    nonisolated static func reapArguments(host: String, apply: Bool) -> [String] {
        var arguments = ["scratch", "reap", "--host", host]
        if apply { arguments.append("--apply") }
        arguments.append("--json")
        return arguments
    }

    /// The exact create command the button will run, from the live form.
    func createArguments(host: String) -> [String] {
        form.arguments(host: host)
    }

    private nonisolated static func present(_ value: String?) -> String? {
        let text = value?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return text.isEmpty ? nil : text
    }

    // MARK: Reads

    func loadProfiles() async {
        guard catalog == nil else { return }
        do {
            let catalog = try await cli.json(
                ScratchProfileCatalog.self,
                arguments: Self.profilesArguments()
            )
            self.catalog = catalog
            if form.profile.isEmpty, let first = catalog.profiles.first {
                form.select(first)
            }
        } catch {
            refusal = Self.message(error)
        }
    }

    func read(host: String) async {
        guard !isReading else { return }
        isReading = true
        defer { isReading = false }
        do {
            let answer = try await cli.jsonResult(
                ScratchLeaseListing.self,
                arguments: Self.listArguments(host: host)
            )
            listing = answer.value
            refusal = answer.refusal
        } catch {
            refusal = Self.message(error)
        }
    }

    // MARK: Writes

    func create(host: String) async {
        guard let receipt = await perform(
            ScratchLeaseReceipt.self,
            arguments: createArguments(host: host),
            working: "Leasing a \(form.profile) scratch target on \(host)",
            summary: Self.createSummary
        ) else { return }
        lease = receipt
        form.name = ""
        await read(host: host)
    }

    func destroy(name: String, host: String) async {
        guard let receipt = await perform(
            ScratchDestroyReceipt.self,
            arguments: Self.destroyArguments(name: name, host: host),
            working: "Destroying the scratch lease \(name) on \(host)",
            summary: Self.destroySummary
        ) else { return }
        destroyed = receipt
        if lease?.name == name { lease = nil }
        await read(host: host)
    }

    func reap(host: String, apply: Bool) async {
        guard let report = await perform(
            ScratchReapReport.self,
            arguments: Self.reapArguments(host: host, apply: apply),
            working: apply
                ? "Destroying every expired scratch lease on \(host)"
                : "Reading which scratch leases on \(host) have expired",
            summary: Self.reapSummary
        ) else { return }
        if apply {
            reapReceipt = report
            reapPlan = nil
            await read(host: host)
        } else {
            reapPlan = report
        }
    }

    func clearMutation() {
        guard !mutation.isWorking else { return }
        mutation = .idle
    }

    /// One write, its receipt, and the CLI's sentence when there is none.
    ///
    /// The deadline is the product command's own: creating an account on a
    /// host and verifying a login into it is a bounded host operation, and a
    /// console-side stopwatch firing halfway through would leave an account
    /// behind a sentence that says nothing about whether it exists.
    private func perform<T: Decodable & Sendable>(
        _ type: T.Type,
        arguments: [String],
        working: String,
        summary: (T) -> String
    ) async -> T? {
        guard !mutation.isWorking else { return nil }
        mutation = .working(working)
        do {
            let answer = try await cli.jsonResult(type, arguments: arguments, timeoutSeconds: nil)
            refusal = answer.refusal
            if let refused = answer.refusal, answer.exitCode != 0 {
                mutation = .failed(refused)
            } else {
                mutation = .succeeded(summary(answer.value))
            }
            return answer.value
        } catch {
            let message = Self.message(error)
            refusal = message
            mutation = .failed(message)
            return nil
        }
    }

    // MARK: What the operator reads back

    nonisolated static func createSummary(_ receipt: ScratchLeaseReceipt) -> String {
        var line = "\(receipt.name) on \(receipt.target): account \(receipt.account.word)"
        line += ", ssh \(receipt.ssh), ttl \(receipt.ttl), expires \(receipt.expiresAt)."
        line += " Storage root \(receipt.storageRoot)."
        if !receipt.reaped.isEmpty {
            line += " Reaped \(receipt.reaped.joined(separator: ", ")) first."
        }
        return line
    }

    nonisolated static func destroySummary(_ receipt: ScratchDestroyReceipt) -> String {
        "\(receipt.name): account \(receipt.account.word), home \(receipt.home.word), "
            + "record \(receipt.record.word) at \(receipt.destroyedAt); "
            + "storage root \(receipt.storageRoot)."
    }

    nonisolated static func reapSummary(_ report: ScratchReapReport) -> String {
        report.apply
            ? "Destroyed \(report.destroyed) expired lease(s) on \(report.target); kept \(report.kept)."
            : "\(report.leases.filter(\.expired).count) expired lease(s) on \(report.target) would be "
                + "destroyed and \(report.kept) kept. Nothing was destroyed by this read."
    }

    /// The CLI's own sentence. A scratch refusal names the declaration, the
    /// profile's platforms or the declared maximum TTL; reworded, it becomes
    /// this console's opinion about a declaration it does not own.
    nonisolated static func message(_ error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
    }

    /// A `--json` command prints one document and nothing else.
    nonisolated static func decode<T: Decodable>(_ type: T.Type, from output: String) -> T? {
        let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let data = trimmed.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(type, from: data)
    }
}
