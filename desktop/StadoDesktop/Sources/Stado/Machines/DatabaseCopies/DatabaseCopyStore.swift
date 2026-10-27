import Combine
import Foundation
import WisentDesignSystem

/// `stado database backup` and `stado database migrate` for the Databases
/// screen: the same commands a terminal runs, behind a confirmation, with the
/// CLI's own report and refusal shown as they come.
@MainActor
final class DatabaseCopyStore: ObservableObject {
    /// One sentence per database the last backup or move answered for.
    @Published private(set) var report: [String] = []
    /// The CLI's own sentence when the last command refused.
    @Published private(set) var problem: String?
    @Published private(set) var isRunning = false

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    /// `stado database backup [NAME] --json`: one database, or every
    /// declared postgres database when `name` is nil.
    nonisolated static func backupArguments(name: String?) -> [String] {
        var arguments = ["database", "backup"]
        if let name {
            arguments.append(name)
        }
        arguments.append("--json")
        return arguments
    }

    /// `stado database migrate NAME --to fleet --port PORT [--host HOST]
    /// --json`. A blank host leaves the choice to the CLI (the vault owner);
    /// a blank port is passed blank, and the CLI refuses naming --port.
    nonisolated static func migrateArguments(name: String, host: String, port: String) -> [String] {
        var arguments = ["database", "migrate", name, "--to", "fleet", "--port", port.trimmingCharacters(in: .whitespaces)]
        let host = host.trimmingCharacters(in: .whitespaces)
        if !host.isEmpty {
            arguments += ["--host", host]
        }
        arguments.append("--json")
        return arguments
    }

    func backup(name: String?) async {
        guard !isRunning else { return }
        isRunning = true
        defer { isRunning = false }
        do {
            let rows = try await cli.json(
                [BackupRow].self, arguments: Self.backupArguments(name: name), confirmsMutation: true
            )
            report = rows.map(\.sentence)
            problem = nil
        } catch {
            problem = error.localizedDescription
        }
    }

    /// `stado database backups --json`: every backup this machine keeps,
    /// newest first; an incomplete one is listed with why.
    nonisolated static func backupsArguments() -> [String] {
        ["database", "backups", "--json"]
    }

    func listBackups() async {
        guard !isRunning else { return }
        isRunning = true
        defer { isRunning = false }
        do {
            let rows = try await cli.json([KeptBackup].self, arguments: Self.backupsArguments())
            report = rows.isEmpty ? ["No backup is kept on this machine."] : rows.map(\.sentence)
            problem = nil
        } catch {
            problem = error.localizedDescription
        }
    }

    /// One row of `stado database backups --json`.
    private struct KeptBackup: Codable {
        let database: String
        let backup: String
        let createdAt: String?
        let rows: Int?
        let incomplete: String?

        enum CodingKeys: String, CodingKey {
            case database, backup, rows, incomplete
            case createdAt = "created_at"
        }

        var sentence: String {
            if let incomplete {
                return "\(database): \(backup) is incomplete: \(incomplete)"
            }
            var parts = [database, backup]
            if let createdAt { parts.append(createdAt) }
            if let rows { parts.append("\(rows) rows") }
            return parts.joined(separator: " · ")
        }
    }

    func migrate(name: String, host: String, port: String) async -> Bool {
        guard !isRunning else { return false }
        isRunning = true
        defer { isRunning = false }
        do {
            let moved = try await cli.json(
                MigrationReport.self,
                arguments: Self.migrateArguments(name: name, host: host, port: port),
                confirmsMutation: true
            )
            report = ["\(moved.migrated) moved to \(moved.to) on \(moved.host); backup \(moved.backup); previous item \(moved.previousItem)"]
            problem = nil
            return true
        } catch {
            problem = error.localizedDescription
            return false
        }
    }

    /// `stado database rest place NAME --port PORT --key-valid-days DAYS
    /// [--rotate-keys] --json`; blank fields are passed blank and the CLI
    /// refuses them by name.
    nonisolated static func restPlaceArguments(name: String, port: String, keyValidDays: String, rotateKeys: Bool) -> [String] {
        var arguments = [
            "database", "rest", "place", name,
            "--port", port.trimmingCharacters(in: .whitespaces),
            "--key-valid-days", keyValidDays.trimmingCharacters(in: .whitespaces),
        ]
        if rotateKeys {
            arguments.append("--rotate-keys")
        }
        arguments.append("--json")
        return arguments
    }

    func placeRest(name: String, port: String, keyValidDays: String, rotateKeys: Bool) async -> Bool {
        guard !isRunning else { return false }
        isRunning = true
        defer { isRunning = false }
        do {
            let placed = try await cli.json(
                RestReport.self,
                arguments: Self.restPlaceArguments(name: name, port: port, keyValidDays: keyValidDays, rotateKeys: rotateKeys),
                confirmsMutation: true
            )
            report = [placed.reused
                ? "\(name): the API is already placed as \(placed.unit); rest_url \(placed.restURL)"
                : "\(name): \(placed.unit) placed; rest_url \(placed.restURL); keys on \(placed.item)"]
            problem = nil
            return true
        } catch {
            problem = error.localizedDescription
            return false
        }
    }

    private struct RestReport: Codable {
        let reused: Bool
        let unit: String
        let restURL: String
        let item: String

        enum CodingKeys: String, CodingKey {
            case reused, unit, item
            case restURL = "rest_url"
        }
    }

    /// `stado database rest retire NAME --json`.
    func retireRest(name: String) async {
        guard !isRunning else { return }
        isRunning = true
        defer { isRunning = false }
        do {
            let retired = try await cli.json(
                RestRetired.self, arguments: ["database", "rest", "retire", name, "--json"], confirmsMutation: true
            )
            report = ["\(name): \(retired.unit) \(retired.unitStatus) on \(retired.host); fields off \(retired.item): \(retired.fieldsRemoved.joined(separator: ", "))"]
            problem = nil
        } catch {
            problem = error.localizedDescription
        }
    }

    private struct RestRetired: Codable {
        let unit: String
        let host: String
        let item: String
        let unitStatus: String
        let fieldsRemoved: [String]

        enum CodingKeys: String, CodingKey {
            case unit, host, item
            case unitStatus = "unit_status"
            case fieldsRemoved = "fields_removed"
        }
    }

    /// One row of `stado database backup --json`: a kept backup, or the
    /// cause it was not taken.
    private struct BackupRow: Codable {
        let database: String
        let backup: String?
        let rows: Int?
        let refused: String?

        var sentence: String {
            if let refused {
                return "\(database): refused: \(refused)"
            }
            if let backup, let rows {
                return "\(database): \(rows) rows kept in \(backup)"
            }
            return "\(database): the CLI answered neither a backup nor a refusal"
        }
    }

    private struct MigrationReport: Codable {
        let migrated: String
        let to: String
        let host: String
        let backup: String
        let previousItem: String

        enum CodingKeys: String, CodingKey {
            case migrated, to, host, backup
            case previousItem = "previous_item"
        }
    }
}
