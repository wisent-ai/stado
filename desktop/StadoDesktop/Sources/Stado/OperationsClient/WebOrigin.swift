import Foundation
import Network

/// A web URL's scheme (RFC 9110 section 4.2), the CLI's
/// `primitives::http::origin::WebScheme`; any other scheme is not a web URL.
enum WebScheme: String {
    case http, https

    init?(scheme: String) {
        self.init(rawValue: scheme.lowercased())
    }
}

/// Whether a URL's host is this machine.
enum WebOrigin {
    static func isSecure(_ scheme: String) -> Bool {
        WebScheme(scheme: scheme) == .https
    }

    static func isWeb(_ scheme: String) -> Bool {
        WebScheme(scheme: scheme) != nil
    }

    /// The `localhost` name or a loopback IP, as the Network framework reads it.
    static func isLoopback(_ host: String) -> Bool {
        if host.caseInsensitiveCompare("localhost") == .orderedSame {
            return true
        }
        if let address = IPv4Address(host) {
            return address.isLoopback
        }
        if let address = IPv6Address(host) {
            return address.isLoopback
        }
        return false
    }
}
