import Foundation
import Network

/// Which transport a URL names, and whether its host is this machine.
///
/// The two web schemes (RFC 9110 section 4.2) are named once here, the same
/// rule the CLI's `primitives::http::origin` applies, so the app's address
/// checks ask this type instead of comparing scheme strings themselves.
enum WebOrigin {
    private static let secure = "https"
    private static let plain = "http"

    static func isSecure(_ scheme: String) -> Bool {
        scheme.lowercased() == secure
    }

    static func isWeb(_ scheme: String) -> Bool {
        isSecure(scheme) || scheme.lowercased() == plain
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
