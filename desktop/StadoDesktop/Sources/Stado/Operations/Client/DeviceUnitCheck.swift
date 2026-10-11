import Foundation

extension DashboardEndpointPreference {
    /// This device's address, after the `com.wisent.stado` LaunchAgent the
    /// address may come from has been read. A unit that exists but cannot be
    /// read, is not a property list or declares no ProgramArguments is an
    /// error naming the file and cause: read as absent, it surfaced as "This
    /// Mac declares no Stado API listener" and sent the operator to declare a
    /// listener the unit already declares.
    static func checkedDeviceURL() throws -> String {
        try DeviceUnit.requireReadable()
        return try deviceURL()
    }
}

extension DashboardEndpointPreference.DeviceUnit {
    static var plistURL: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/LaunchAgents/com.wisent.stado.plist")
    }

    /// Refuses a present LaunchAgent that cannot be used; a missing one is a
    /// Mac without the unit, which `deviceURL` already reports.
    static func requireReadable() throws {
        let plist = plistURL
        guard FileManager.default.fileExists(atPath: plist.path) else { return }
        let data: Data
        do {
            data = try Data(contentsOf: plist)
        } catch {
            throw StadoCLIError.failed(
                exitCode: nil,
                message: "The Stado LaunchAgent at \(plist.path) cannot be read: \(error.localizedDescription)"
            )
        }
        let decoded: Any
        do {
            decoded = try PropertyListSerialization.propertyList(from: data, format: nil)
        } catch {
            throw StadoCLIError.failed(
                exitCode: nil,
                message: "The Stado LaunchAgent at \(plist.path) is not a property list: \(error.localizedDescription)"
            )
        }
        guard let root = decoded as? [String: Any], root["ProgramArguments"] is [String] else {
            throw StadoCLIError.failed(
                exitCode: nil,
                message: "The Stado LaunchAgent at \(plist.path) declares no ProgramArguments list."
            )
        }
    }
}
