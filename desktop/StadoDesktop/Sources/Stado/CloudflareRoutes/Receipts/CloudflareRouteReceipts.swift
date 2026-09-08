import Foundation

/// What Stado reports after it has changed something, decoded exactly as the
/// CLI prints it.

/// The nonsecret receipt printed by `stado cloudflare route-tunnel --json`.
struct CloudflareRouteReceipt: Decodable, Sendable {
    let status: String
    let action: String
    let zone: String
    let hostname: String
    let origin: String
    let dnsContent: String
    let proxied: Bool
    let connectorHost: String
    let connectorService: String
    let connectorUnit: String
    let connectorSecretPath: String
    let connectorRestart: String

    enum CodingKeys: String, CodingKey {
        case status
        case action
        case zone
        case hostname
        case origin
        case proxied
        case dnsContent = "dns_content"
        case connectorHost = "connector_host"
        case connectorService = "connector_service"
        case connectorUnit = "connector_unit"
        case connectorSecretPath = "connector_secret_path"
        case connectorRestart = "connector_restart"
    }
}

struct CloudflareRouteRemovalReceipt: Decodable, Sendable {
    let status: String
    let zone: String
    let hostname: String
    let dnsContent: String
    let removedDNSRecords: Int
    let removedIngressRules: Int
    let connectorPreserved: Bool

    enum CodingKeys: String, CodingKey {
        case status
        case zone
        case hostname
        case dnsContent = "dns_content"
        case removedDNSRecords = "removed_dns_records"
        case removedIngressRules = "removed_ingress_rules"
        case connectorPreserved = "connector_preserved"
    }
}
