import Foundation

/// One hostname inspected against tunnel ingress, DNS and active connectors.
struct CloudflareRouteState: Decodable, Identifiable, Equatable, Sendable {
    let hostname: String
    let origin: String?
    let ingressRules: Int
    let dnsRecords: Int
    let conflictingDNSRecords: Int
    let dnsRecordIDs: [String]
    let dnsContent: String
    let proxied: Bool
    let tunnelConnected: Bool
    let consistent: Bool
    let state: String
    let originReachability: String

    var id: String { hostname }

    enum CodingKeys: String, CodingKey {
        case hostname
        case origin
        case ingressRules = "ingress_rules"
        case dnsRecords = "dns_records"
        case conflictingDNSRecords = "conflicting_dns_records"
        case dnsRecordIDs = "dns_record_ids"
        case dnsContent = "dns_content"
        case proxied
        case tunnelConnected = "tunnel_connected"
        case consistent
        case state
        case originReachability = "origin_reachability"
    }
}
