import Foundation

/// What the operator has typed into the Memory screen's declaration editor,
/// before any of it is a write.
///
/// Seeded from the state the screen read, so an unedited draft produces no
/// patch at all and the review action stays disabled.
struct MemoryPolicyDraft: Sendable {
    var mode: MemoryReclaimMode?
    var numbers: [MemoryReclaimNumericField: String]
    var refusePlacement: Bool
    var repairsText: String {
        didSet { repairsDocument = Self.parseRepairs(repairsText) }
    }
    private(set) var repairsDocument: StorageReconciliationJSON?

    init(state: MemoryPolicyState) {
        mode = state.mode
        numbers = Dictionary(
            uniqueKeysWithValues: MemoryReclaimNumericField.allCases.map { field in
                (field, state.value(of: field).map(String.init) ?? "")
            }
        )
        refusePlacement = state.refusePlacement
        let document = state.repairsDocument
        repairsText = document.prettyJSON
        repairsDocument = document
    }

    func text(for field: MemoryReclaimNumericField) -> String {
        numbers[field] ?? ""
    }

    private static func parseRepairs(_ text: String) -> StorageReconciliationJSON? {
        guard let value = try? JSONDecoder().decode(StorageReconciliationJSON.self, from: Data(text.utf8)),
              value.objectValue != nil else { return nil }
        return value
    }

    var validationError: String? {
        if repairsDocument == nil { return "Repairs must be a JSON object." }
        for field in MemoryReclaimNumericField.allCases {
            if !text(for: field).trimmingCharacters(in: .whitespaces).isEmpty, value(for: field) == nil {
                return "\(field.title) must be a positive whole number within its supported range."
            }
        }
        return nil
    }

    /// The typed value, when it is a positive whole number. A percentage is
    /// bounded too, because swap cannot be more than wholly used and a larger
    /// number is a typo rather than a watermark.
    func value(for field: MemoryReclaimNumericField) -> Int? {
        guard let typed = Int(text(for: field).trimmingCharacters(in: .whitespaces)),
              typed > Int.zero
        else { return nil }
        if field == .highSwapUsedPct, typed > MemoryReclaimPatch.wholePercent { return nil }
        return typed
    }
}

/// One whitelisted `memory_reclaim` patch: exactly the fields the operator
/// changed, in one canonical order, so the review dialog shows the bytes that
/// will be posted rather than a paraphrase of them.
///
/// Repair declarations are included in the reviewed patch and validated by the
/// same backend as CLI changes; this screen does not invent another policy.
struct MemoryReclaimPatch: Sendable {
    /// The upper bound of a swap-utilisation watermark.
    static let wholePercent = 100
    /// The registry key this patch is written under.
    static let registryKey = "memory_reclaim"

    var mode: MemoryReclaimMode?
    var numbers: [MemoryReclaimNumericField: Int]
    var refusePlacement: Bool?
    var repairs: StorageReconciliationJSON?
    let authorizesRepairs: Bool

    /// The patch a draft represents against the state it was seeded from, or
    /// `nil` when the operator has changed nothing.
    init?(draft: MemoryPolicyDraft, current: MemoryPolicyState) {
        guard draft.validationError == nil else { return nil }
        guard let repairDocument = draft.repairsDocument else { return nil }
        let repairs = repairDocument.prettyJSON == current.repairsDocument.prettyJSON ? nil : repairDocument
        var mode: MemoryReclaimMode?
        if let drafted = draft.mode, drafted != current.mode {
            mode = drafted
        }
        var numbers: [MemoryReclaimNumericField: Int] = [:]
        for field in MemoryReclaimNumericField.allCases {
            guard let value = draft.value(for: field), value != current.value(of: field) else { continue }
            numbers[field] = value
        }
        let refusal: Bool? = draft.refusePlacement == current.refusePlacement
            ? nil
            : draft.refusePlacement
        guard mode != nil || !numbers.isEmpty || refusal != nil || repairs != nil else { return nil }
        self.mode = mode
        self.numbers = numbers
        refusePlacement = refusal
        self.repairs = repairs
        authorizesRepairs = draft.mode == .enforce
    }

    /// Mode first, then the watermarks in the order a pass applies them, then
    /// the budget, then the placement refusal: the order the schema declares
    /// them and the order an operator reads them.
    private var orderedEntries: [(key: String, rendered: String)] {
        var entries: [(key: String, rendered: String)] = []
        if let mode {
            entries.append((MemoryPatchKey.mode, "\"\(mode.rawValue)\""))
        }
        for field in MemoryReclaimNumericField.allCases {
            guard let value = numbers[field] else { continue }
            entries.append((field.rawValue, String(value)))
        }
        if let refusePlacement {
            entries.append((MemoryPatchKey.refusePlacement, refusePlacement ? "true" : "false"))
        }
        if let repairs { entries.append(("repairs", repairs.prettyJSON)) }
        return entries
    }

    /// The `memory_reclaim` object this patch posts.
    var fields: [String: Any] {
        var fields: [String: Any] = [:]
        if let mode {
            fields[MemoryPatchKey.mode] = mode.rawValue
        }
        for (field, value) in numbers {
            fields[field.rawValue] = value
        }
        if let refusePlacement {
            fields[MemoryPatchKey.refusePlacement] = refusePlacement
        }
        if let repairs { fields["repairs"] = repairs.foundationValue }
        return fields
    }

    /// The exact body `POST /api/registry/policy` receives, rendered for the
    /// review dialog. The client serializes the same fields; this is the one
    /// place their order is fixed, because a reviewed patch and a posted
    /// patch that read differently are two patches.
    func canonicalJSON(target: String) -> String {
        let inner = orderedEntries
            .map { "\"\($0.key)\": \($0.rendered)" }
            .joined(separator: ", ")
        return "{\"target\": \"\(target)\", \"\(Self.registryKey)\": {\(inner)}}"
    }

    /// One operator sentence per changed field, for the review dialog's
    /// listing: what is being written, and what it does when the host reads it.
    var reviewLines: [String] {
        var lines: [String] = []
        if let mode {
            lines.append("mode → \(mode.rawValue): \(mode.effect)")
        }
        for field in MemoryReclaimNumericField.allCases {
            guard let value = numbers[field] else { continue }
            lines.append("\(field.rawValue) → \(value): \(field.effect)")
        }
        if let refusePlacement {
            lines.append(
                refusePlacement
                    ? "refuse_placement → true: while this host is over a watermark it stops accepting new jobs, with \(MemoryReclaimReport.admissionReason) as the recorded admission reason."
                    : "refuse_placement → false: the host keeps accepting jobs while it is over its watermark, and the pass only reports and repairs."
            )
        }
        if let repairs {
            lines.append("Replace the declared memory repairs with: \(repairs.prettyJSON)")
        }
        return lines
    }

}

/// The two patch keys that are not a numeric field's own name.
enum MemoryPatchKey {
    static let mode = "mode"
    static let refusePlacement = "refuse_placement"
}
