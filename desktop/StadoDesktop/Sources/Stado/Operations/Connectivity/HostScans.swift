import Foundation
import WisentDesignSystem

// MARK: - A host's scans of its own disk

/// The `scans` block of a host's beacon, as `stado host link <host> --json`
/// carries it.
///
/// A host's own disk scans — the beacon's launchd event reads, the janitor's
/// build-cache walk, the inventory walk of a space report on its own host —
/// are admitted against one load ceiling, and a skipped scan is published
/// here instead of silently not happening. Every field is the beacon's own
/// answer; a scan absent from `skipped` ran when it last asked.
struct HostScans: Decodable, Sendable {
    /// The reading the latest admission was decided on; `nil` while no scan
    /// has asked yet in the publisher's life.
    let last: HostScanReading?
    /// Every scan standing skipped, by the name the beacon publishes it under
    /// (`unit_events`, `build_caches`, `disk_inventory`).
    let skipped: [String: HostScanSkip]

    /// The one line the Link section renders: the reading against its
    /// ceiling, then every standing skip and since when.
    var line: String {
        guard let last else { return "No scan has asked yet" }
        var sentence = "load \(last.load.formatted()) against ceiling \(last.ceiling.formatted()) (\(last.ceilingSource))"
        for (scan, skip) in skipped.sorted(by: { $0.key < $1.key }) {
            sentence += "; \(scan) skipped since \(skip.since)"
        }
        return sentence
    }

    /// Whether any scan stands skipped: the host is above its ceiling and
    /// the readings those scans produce are not being refreshed.
    var anySkipped: Bool { !skipped.isEmpty }

    /// The Link section's field: the block's line, or that a beacon from
    /// before the block has not reported it.
    static func description(of scans: HostScans?) -> String {
        guard let scans else { return "Not reported" }
        return scans.line
    }

    /// A standing skip is worth the operator's eye: the host is above its
    /// ceiling and its readings are not being refreshed.
    static func tone(of scans: HostScans?) -> WisentTone {
        guard let scans else { return .neutral }
        return scans.anySkipped ? .warning : .neutral
    }
}

/// One admission's reading: the host's one-minute load average against the
/// ceiling and where that ceiling came from (`declared` or `measured_cores`).
struct HostScanReading: Decodable, Sendable {
    let load: Double
    let ceiling: Double
    let ceilingSource: String

    enum CodingKeys: String, CodingKey {
        case load, ceiling
        case ceilingSource = "ceiling_source"
    }
}

/// One scan standing skipped: the latest refusal's reading, when the run of
/// refusals began and when its newest one was.
struct HostScanSkip: Decodable, Sendable {
    let since: String
    let latest: String
    let load: Double
    let ceiling: Double
    let ceilingSource: String

    enum CodingKeys: String, CodingKey {
        case since, latest, load, ceiling
        case ceilingSource = "ceiling_source"
    }
}
