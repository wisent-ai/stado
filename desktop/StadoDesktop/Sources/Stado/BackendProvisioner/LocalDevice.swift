import Foundation

// MARK: - This device

extension BackendProvisioner {
    func provisionLocal(
        deployment: StadoDeployment,
        onUpdate: UpdateHandler
    ) async throws -> ProvisionedBackend {
        await onUpdate(.init(phase: "Preparing this device", detail: "Creating an isolated deployment directory", fraction:
            0.15))
        let executable = try locateStadoCLI()
        let support = try fileManager.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        let deploymentRoot = support
            .appendingPathComponent("Stado", isDirectory: true)
            .appendingPathComponent("Deployments", isDirectory: true)
            .appendingPathComponent(deployment.id, isDirectory: true)
        let storageRoot = deploymentRoot.appendingPathComponent("storage", isDirectory: true)
        try fileManager.createDirectory(at: storageRoot, withIntermediateDirectories: true)

        let port = stablePort(for: deployment.id)
        let endpoint = "http://127.0.0.1:\(port)"
        let label = "ai.wisent.stado.deployment.\(deployment.id.lowercased().replacingOccurrences(of: "-", with: ""))"
        let launchAgents = fileManager.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/LaunchAgents", isDirectory: true)
        try fileManager.createDirectory(at: launchAgents, withIntermediateDirectories: true)
        let plistURL = launchAgents.appendingPathComponent("\(label).plist")
        let logs = deploymentRoot.appendingPathComponent("logs", isDirectory: true)
        try fileManager.createDirectory(at: logs, withIntermediateDirectories: true)

        let environment = ProcessInfo.processInfo.environment
        let plist: [String: Any] = [
            "Label": label,
            "ProgramArguments": [
                executable.path,
                "local-control-plane",
                "--bind", "127.0.0.1",
                "--port", String(port),
                "--interval", "15"
            ],
            "EnvironmentVariables": [
                "PATH": environment["PATH"] ?? "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
                "HOME": fileManager.homeDirectoryForCurrentUser.path,
                "WC_STORAGE_BACKEND": "local",
                "WC_LOCAL_STORAGE_PATH": storageRoot.path,
                "WC_BUCKET": "stado-\(deployment.id)",
                "WC_PROVIDERS": "local",
                "WC_DASHBOARD_REFRESH_SECONDS": "5",
                "STADO_DEPLOYMENT_ID": deployment.id,
            ],
            "RunAtLoad": true,
            "KeepAlive": ["SuccessfulExit": false],
            "ThrottleInterval":
                5,
            "StandardOutPath": logs.appendingPathComponent("service.log").path,
            "StandardErrorPath": logs.appendingPathComponent("service-error.log").path
        ]
        let data = try PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options:
            0)
        try data.write(to: plistURL, options: .atomic)

        await onUpdate(.init(phase: "Starting Stado", detail: "Installing the per-user control-plane service", fraction:
            0.45))
        let domain = "gui/\(getuid())"
        _ = try? await run("/bin/launchctl", ["bootout", "\(domain)/\(label)"])
        do {
            try await run("/bin/launchctl", ["bootstrap", domain, plistURL.path])
            try await run("/bin/launchctl", ["kickstart", "-k", "\(domain)/\(label)"])
        } catch {
            throw BackendProvisioningError.commandFailed(error.localizedDescription)
        }

        await onUpdate(.init(phase: "Checking health", detail: endpoint, fraction:
            0.75))
        try await waitUntilHealthy(endpoint: endpoint)
        await onUpdate(.init(phase: "Ready", detail: "This device is running the Stado backend", fraction:
            1))
        return ProvisionedBackend(endpoint: endpoint, region: "This Mac")
    }
}
