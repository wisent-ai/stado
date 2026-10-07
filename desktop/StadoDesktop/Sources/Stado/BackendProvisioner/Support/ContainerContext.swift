import Foundation

// MARK: - Container context

extension BackendProvisioner {
    /// Package the exact release installer returned by the chosen Stado API.
    /// The image downloads and verifies the Linux release; Desktop does not
    /// invoke a local Stado executable to obtain it.
    func prepareContainerContext(installer: String, cadence: ServeCadence) throws -> URL {
        guard installer.contains("release_version=") else {
            throw BackendProvisioningError.commandFailed(
                "The Stado API did not return an exact release installer."
            )
        }
        let context = fileManager.temporaryDirectory
            .appendingPathComponent("stado-cloud-\(UUID().uuidString)", isDirectory: true)
        try fileManager.createDirectory(at: context, withIntermediateDirectories: true)
        try installer.write(
            to: context.appendingPathComponent("install.sh"),
            atomically: true,
            encoding: .utf8
        )
        let dockerfile = """
        FROM debian:bookworm-slim
        RUN apt-get update \\
         && apt-get install -y --no-install-recommends bash ca-certificates curl \\
         && rm -rf /var/lib/apt/lists/*
        COPY install.sh /tmp/install.sh
        RUN bash /tmp/install.sh && rm /tmp/install.sh
        CMD ["/root/.stado/bin/stado", "serve", "--control-plane", "cloud", "--control-plane-interval-seconds", "\(cadence.controlPlaneIntervalSeconds)", "--api", "--bind", "0.0.0.0", "--port", "8080"]
        """
        try dockerfile.write(
            to: context.appendingPathComponent("Dockerfile"),
            atomically: true,
            encoding: .utf8
        )
        return context
    }
}
