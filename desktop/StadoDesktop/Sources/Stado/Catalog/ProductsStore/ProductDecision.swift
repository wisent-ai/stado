struct ProductDecision: Identifiable {
    let verb: String
    let product: String
    let productName: String
    let surface: String
    let host: String?
    let destructive: Bool
    let releaseVersion: String?
    let sourceCommit: String?
    let withPrerequisites: Bool

    var id: String {
        if let host { return "\(verb)/\(product)/\(surface)/\(host)" }
        return "\(verb)/\(product)/\(surface)"
    }
}
