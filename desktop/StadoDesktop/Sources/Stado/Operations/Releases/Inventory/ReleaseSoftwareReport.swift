import Foundation

/// One product target as the inventory lists it: its identity, and the software
/// report the CLI attached to it.
struct ReleaseInventoryEntry: Decodable, Sendable {
    let pair: ReleaseInventoryPair
    /// Absent only when the CLI predates the software half of the report. It is
    /// carried as `nil` rather than as a passing verdict, because "this console
    /// is newer than the CLI" and "the host is accounted for" are different
    /// facts and only one of them is good news.
    let software: ReleaseSoftwareReport?

    enum CodingKeys: String, CodingKey {
        case software
    }

    init(from decoder: Decoder) throws {
        pair = try ReleaseInventoryPair(from: decoder)
        let values = try decoder.container(keyedBy: CodingKeys.self)
        software = try values.decodeIfPresent(ReleaseSoftwareReport.self, forKey: .software)
    }
}

/// What a host said it runs, and whether the CLI could account for it.
///
/// `state` and `verdict` are two different questions and stay two fields.
/// `state` is whether anybody looked — `observed`, `unverified`, `never`.
/// `verdict` is what the look implies — `ok` or `failed`. Folding them would let
/// this screen paint "nobody has ever asked this host" in the same colour as
/// "the host answered and it is fine", which is the exact reading that let a
/// stale skarbiec strip a live subscription's tags for a day.
struct ReleaseSoftwareReport: Decodable, Sendable {
    let state: String
    let verdict: String
    let failed: Bool
    /// `just now`, `14m ago`, `stale (3h)`, `never` — the CLI's own phrase.
    let observed: String
    /// The counts the host's report carries, as the CLI gave them.
    let reported: Int?
    let release: Int?
    let unmanaged: Int?
    let scripts: Int?
    /// Verbatim, in the CLI's words, in the CLI's order. Never re-worded here.
    let findings: [String]

    var hasReport: Bool { state != "never" }

    /// `state`, `observed`, `verdict`, `failed` and `findings` are written in
    /// every software block the CLI attaches to a target: the host's own
    /// report with the verdict folded in, for a target the registry does not
    /// declare as much as for a judged one. A block that lacks one is a
    /// decoding error naming the key, never a report or verdict this screen
    /// made up. The counts are absent only in a block an older CLI wrote,
    /// and are carried as absent, shown as not reported.
    enum CodingKeys: String, CodingKey {
        case state, verdict, failed, observed, reported, release, unmanaged, scripts, findings
    }
}

struct ReleaseInventoryPair: Decodable, Identifiable, Hashable, Sendable {
    let product: String
    let target: String

    var id: String { "\(product)/\(target)" }

    enum CodingKeys: String, CodingKey {
        case product, target
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        product = try values.decode(String.self, forKey: .product)
        target = try values.decode(String.self, forKey: .target)
    }

    init(product: String, target: String) {
        self.product = product
        self.target = target
    }
}
