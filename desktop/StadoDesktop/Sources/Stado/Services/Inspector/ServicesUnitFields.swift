import SwiftUI
import WisentDesignSystem

/// The declared-unit and unowned-process inspectors, and the small readers the
/// rows share with them.
///
/// `unitInspector` and `unownedInspector` are internal rather than private
/// because `inspector` sits in `ServicesInspector.swift`; `isLongLived` and
/// `value` because the row tables sit in `Rows/`: Swift scopes `private` to
/// one file.
extension ServicesView {
    func unitInspector(_ row: ServiceUnitRow) -> some View {
        let unit = row.unit
        return WisentInspector(
            eyebrow: "Declared unit",
            title: unit.unit.isEmpty ? unit.binary : unit.unit,
            badges: badges(for: unit)
        ) {
            if unit.servesReplacedCode {
                WisentAlertPanel(
                    tone: .danger,
                    title: "The process is not executing the program on disk",
                    detail: "The unit runs, and the binary the process is executing is not the one under the directory this unit declares. Whatever was fixed in the program on disk is not what this host is serving, and nothing will change that until the unit restarts."
                )
            } else if unit.binaryMatchesProcess == nil {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The host did not say which binary the process runs",
                    detail: "This host reported no running-binary comparison, so whether the process is executing the program on disk is unknown here. It is not a claim that they match."
                )
            }
            WisentField(label: "Host", value: row.host)
            WisentField(label: "Binary", value: value(unit.binary))
            WisentField(label: "Unit state", value: value(unit.state))
            WisentField(label: "Verdict", value: value(unit.verdict))
            WisentField(label: "Declared program", value: value(unit.root))
            WisentField(
                label: "Running binary",
                value: unit.runningBinary ?? "Not reported",
                tone: unit.servesReplacedCode ? .danger : .neutral
            )
            WisentField(
                label: "Process matches program on disk",
                value: matchDescription(unit),
                tone: unit.servesReplacedCode ? .danger : (unit.binaryMatchesProcess == nil ? .warning : .success)
            )
            WisentField(label: "Declared version", value: value(unit.declaredVersion))
            WisentField(label: "Installed version", value: value(unit.installedVersion))
            if !unit.detail.isEmpty {
                WisentField(label: "Detail", value: unit.detail)
            }
        }
    }

    func unownedInspector(_ process: UnownedProcess) -> some View {
        WisentInspector(
            eyebrow: "Owned by no unit",
            title: process.productGuess ?? "Unidentified process",
            badges: [("PID \(value(process.pid))", .warning)]
        ) {
            WisentAlertPanel(
                tone: .warning,
                title: "Nothing supervises this process",
                detail: "No declared unit owns it, so no release updates it, nothing restarts it if it dies, and nothing stops it. Two processes in this state ran for four days before anybody looked. Ending it is a decision for whoever knows what it is doing, and this console does not make it."
            )
            WisentField(label: "Host", value: process.host)
            WisentField(label: "PID", value: value(process.pid))
            WisentField(
                label: "Started",
                value: process.startedAt ?? "Not reported",
                tone: isLongLived(process) ? .warning : .neutral
            )
            WisentField(
                label: "Running for",
                value: process.age == nil
                    ? "The host's start stamp could not be read here, so the age is unknown — the stamp above is what it said"
                    : StadoFormat.duration(process.age),
                tone: isLongLived(process) ? .warning : .neutral
            )
            WisentField(
                label: "Product guess",
                value: process.productGuess ?? "Nothing declared this process, so nothing knows what it is"
            )
            WisentField(label: "Command", value: value(process.command))
        }
    }

    /// A day. The four-day agent processes are the case this screen was built
    /// for, and a process nothing owns that has been up since yesterday is
    /// already past the point where somebody meant to start it by hand.
    func isLongLived(_ process: UnownedProcess) -> Bool {
        (process.age ?? 0) > 86_400
    }

    private func badges(for unit: ServiceUnit) -> [(String, WisentTone)] {
        var values: [(String, WisentTone)] = []
        if !unit.state.isEmpty {
            values.append((unit.state, .neutral))
        }
        if unit.servesReplacedCode {
            values.append(("Serving replaced code", .danger))
        }
        return values
    }

    private func matchDescription(_ unit: ServiceUnit) -> String {
        switch unit.binaryMatchesProcess {
        case true: "Yes — the process runs the program under the declared directory"
        case false: "No — the process runs a different binary"
        case nil: "Not reported by this host"
        }
    }

    /// A blank cell reads as "there is nothing wrong here"; a missing value is
    /// not that.
    func value(_ text: String?) -> String {
        guard let text = text?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else {
            return "Not reported"
        }
        return text
    }
}
