import Foundation

/// The literal defaults the Cloudflare route screen ships, kept in a constants
/// module so each value is named once where it is declared and used by name at
/// every point that renders or sends it.
enum CloudflareRouteConstants {
    /// The connector-local origin a fresh draft starts from, and the same text
    /// shown as the hostname form's origin placeholder. A connector reaches
    /// its service over loopback, so the default names loopback explicitly
    /// rather than leaving the field empty.
    static let defaultOrigin = "http://localhost:3000"
}
