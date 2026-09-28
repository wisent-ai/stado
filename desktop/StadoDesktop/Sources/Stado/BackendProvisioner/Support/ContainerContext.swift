import Foundation

// MARK: - Container context

extension BackendProvisioner {
    /// The cloud control plane's image context: the Stado release installer
    /// this Stado prints (`stado bootstrap --print-install-script`) and a
    /// Dockerfile that runs it. The image downloads the Linux release of the
    /// same version from the public release route, checks its manifest and
    /// digest, and runs `stado cloud-control-plane`; nothing is copied from
    /// this Mac, whose binary is not a Linux one.
    func prepareContainerContext(stadoExecutable: URL) async throws -> URL {
        let installer = try await runCapture(stadoExecutable.path, ["bootstrap", "--print-install-script"])
        guard installer.contains("release_version=") else {
            throw BackendProvisioningError.commandFailed(
                "This Stado could not print its release installer (stado bootstrap --print-install-script)."
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
        CMD ["/root/.stado/bin/stado", "cloud-control-plane", "--bind", "0.0.0.0", "--port", "8080", "--interval", "30"]
        """
        try dockerfile.write(
            to: context.appendingPathComponent("Dockerfile"),
            atomically: true,
            encoding: .utf8
        )
        return context
    }
}
