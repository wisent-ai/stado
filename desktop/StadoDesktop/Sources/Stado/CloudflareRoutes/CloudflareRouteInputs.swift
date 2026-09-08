import Foundation

/// Everything the operator chooses before a Cloudflare command runs: the
/// credential item ids, the zone, and the full `route-tunnel` draft.
///
/// `CloudflareRouteValidation` stays private to this file because its only two
/// callers are the scope and the draft declared beside it.

/// Nonsecret metadata returned by `stado credentials ls --json`.
///
/// Cloudflare operations use item ids only. Secret fields never enter SwiftUI
/// state, rendered commands, inventory rows, or receipts.
struct CloudflareCredentialItem: Decodable, Identifiable, Sendable {
    let id: String
}

/// The Cloudflare account, tunnel and zone whose routes are being managed.
struct CloudflareRouteScope: Equatable, Sendable {
    var apiCredential = ""
    var tunnelCredential = ""
    var zone = ""

    var normalized: Self {
        Self(
            apiCredential: apiCredential.trimmingCharacters(in: .whitespacesAndNewlines),
            tunnelCredential: tunnelCredential.trimmingCharacters(in: .whitespacesAndNewlines),
            zone: zone.trimmingCharacters(in: .whitespacesAndNewlines)
        )
    }

    var problems: [String] {
        let value = normalized
        var result: [String] = []
        if value.apiCredential.isEmpty {
            result.append("Choose the credential containing account_id and api_token.")
        }
        if value.tunnelCredential.isEmpty {
            result.append("Choose the credential containing account_id, tunnel_id and the connector token.")
        }
        if !CloudflareRouteValidation.isDNSName(value.zone) {
            result.append("Zone must be a lowercase DNS name.")
        }
        return result
    }

    var listArguments: [String] {
        let value = normalized
        return [
            "cloudflare", "list",
            "--api-credential", value.apiCredential,
            "--tunnel-credential", value.tunnelCredential,
            "--zone", value.zone,
            "--json",
        ]
    }

    func statusArguments(hostname: String) -> [String] {
        let value = normalized
        return [
            "cloudflare", "status",
            "--api-credential", value.apiCredential,
            "--tunnel-credential", value.tunnelCredential,
            "--zone", value.zone,
            "--hostname", hostname,
            "--json",
        ]
    }

    func removeArguments(hostname: String) -> [String] {
        let value = normalized
        return [
            "cloudflare", "remove",
            "--api-credential", value.apiCredential,
            "--tunnel-credential", value.tunnelCredential,
            "--zone", value.zone,
            "--hostname", hostname,
            "--json",
        ]
    }
}

/// Every input owned by `stado cloudflare route-tunnel`.
///
/// Keeping the defaults explicit makes the command shown in the window exactly
/// the command that runs, even if a later CLI release changes a default.
struct CloudflareRouteDraft: Equatable, Sendable {
    var apiCredential = ""
    var tunnelCredential = ""
    var zone = ""
    var hostname = ""
    var origin = CloudflareRouteConstants.defaultOrigin
    var host = ""
    var connectorService = "cloudflared"
    var connectorTokenField = "token"
    var connectorSecretName = "cloudflared-token"

    var scope: CloudflareRouteScope {
        CloudflareRouteScope(
            apiCredential: apiCredential,
            tunnelCredential: tunnelCredential,
            zone: zone
        )
    }

    var normalized: Self {
        var value = self
        value.apiCredential = apiCredential.trimmingCharacters(in: .whitespacesAndNewlines)
        value.tunnelCredential = tunnelCredential.trimmingCharacters(in: .whitespacesAndNewlines)
        value.zone = zone.trimmingCharacters(in: .whitespacesAndNewlines)
        value.hostname = hostname.trimmingCharacters(in: .whitespacesAndNewlines)
        value.origin = origin.trimmingCharacters(in: .whitespacesAndNewlines)
        value.host = host.trimmingCharacters(in: .whitespacesAndNewlines)
        value.connectorService = connectorService.trimmingCharacters(in: .whitespacesAndNewlines)
        value.connectorTokenField = connectorTokenField.trimmingCharacters(in: .whitespacesAndNewlines)
        value.connectorSecretName = connectorSecretName.trimmingCharacters(in: .whitespacesAndNewlines)
        return value
    }

    var arguments: [String] {
        let value = normalized
        return [
            "cloudflare", "route-tunnel",
            "--api-credential", value.apiCredential,
            "--tunnel-credential", value.tunnelCredential,
            "--zone", value.zone,
            "--hostname", value.hostname,
            "--origin", value.origin,
            "--host", value.host,
            "--connector-service", value.connectorService,
            "--connector-token-field", value.connectorTokenField,
            "--connector-secret-name", value.connectorSecretName,
            "--json",
        ]
    }

    /// Fast form feedback mirrors the CLI's public input contract. The CLI
    /// remains authoritative and its own refusal is shown if the two differ.
    var problems: [String] {
        let value = normalized
        var result = value.scope.problems
        if !CloudflareRouteValidation.isDNSName(value.hostname) {
            result.append("Hostname must be a lowercase DNS name.")
        } else if !value.zone.isEmpty,
                  value.hostname != value.zone,
                  !value.hostname.hasSuffix(".\(value.zone)") {
            result.append("Hostname must be inside the selected zone.")
        }
        if !CloudflareRouteValidation.isHTTPOrigin(value.origin) {
            result.append("Origin must be an HTTP(S) URL without credentials or a fragment.")
        }
        if value.host.isEmpty {
            result.append("Choose the registry host running the connector.")
        }
        if value.connectorService.isEmpty {
            result.append("Connector service is required.")
        }
        if value.connectorTokenField.isEmpty {
            result.append("Connector token field is required.")
        }
        if value.connectorSecretName.isEmpty {
            result.append("Connector secret filename is required.")
        }
        return result
    }
}

private enum CloudflareRouteValidation {
    static func isDNSName(_ value: String) -> Bool {
        guard !value.isEmpty,
              value.utf8.count <= 253,
              value == value.lowercased()
        else { return false }
        return value.split(separator: ".", omittingEmptySubsequences: false).allSatisfy { label in
            guard !label.isEmpty,
                  label.utf8.count <= 63,
                  label.first != "-",
                  label.last != "-"
            else { return false }
            return label.utf8.allSatisfy { byte in
                (byte >= 0x61 && byte <= 0x7A)
                    || (byte >= 0x30 && byte <= 0x39)
                    || byte == 0x2D
            }
        }
    }

    static func isHTTPOrigin(_ value: String) -> Bool {
        guard let parsed = URLComponents(string: value),
              let scheme = parsed.scheme,
              scheme == "http" || scheme == "https",
              parsed.host?.isEmpty == false,
              parsed.user == nil,
              parsed.password == nil,
              parsed.fragment == nil
        else { return false }
        return true
    }
}
