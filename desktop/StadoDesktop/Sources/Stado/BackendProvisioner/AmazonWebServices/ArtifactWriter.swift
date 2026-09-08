import Foundation

// MARK: - Amazon Web Services artifacts

/// Writes one CLI input document into a prepared container context.
///
/// The AWS run is long enough to live in two files, and both halves hand the
/// `aws` CLI JSON documents out of the same context directory. Carrying the
/// directory in a value with `callAsFunction` lets every `writeJSON(_:named:)`
/// call site read exactly as it did when the run was one function.
struct AWSArtifactWriter: Sendable {
    let directory: URL

    func callAsFunction(_ object: Any, named name: String) throws -> URL {
        let url = directory.appendingPathComponent(name)
        try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted])
            .write(to: url, options: .atomic)
        return url
    }
}
