import CryptoKit
import Foundation

// MARK: - Endpoint

extension BackendProvisioner {
    func stablePort(for deploymentID: String) -> Int {
        let digest = SHA256.hash(data: Data(deploymentID.utf8))
        let value = digest.prefix(2).reduce(0) { ($0 << 8) | Int($1) }
        let port = 8800 + value % 1000
        return port
    }

    func waitUntilHealthy(endpoint: String) async throws {
        guard let url = URL(string: endpoint + "/healthz") else {
            throw BackendProvisioningError.healthCheckFailed(endpoint)
        }
        for _ in 0..<120 {
            do {
                var request = URLRequest(url: url)
                request.timeoutInterval = 2
                let (_, response) = try await session.data(for: request)
                if let http = response as? HTTPURLResponse, http.statusCode == 200 { return }
            } catch {
                // Launching Python and importing provider SDKs can take a few seconds.
            }
            try await Task.sleep(for: .seconds(1))
        }
        throw BackendProvisioningError.healthCheckFailed(endpoint)
    }
}
