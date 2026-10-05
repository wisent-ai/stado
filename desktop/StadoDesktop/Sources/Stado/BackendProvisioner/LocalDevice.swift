import Foundation

// MARK: - This device

extension BackendProvisioner {
    /// The exit status `launchctl print` gives for a service launchd does not
    /// hold (`Could not find service`, EX_NOTFOUND in launchd's own codes).
    static let launchdServiceNotFound: Int32 = 113

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

        // The listener binds a port the system assigns and announces the
        // address it bound; the endpoint is read from that announcement.
        // Stado runs on a host as one process, com.wisent.stado; a deployment
        // on this Mac is that process with this deployment's storage.
        let label = Self.stadoUnit
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
                "serve",
                "--standalone",
                "--worker",
                "--kind", "local",
                "--poll-seconds", "15",
                "--control-plane", "local",
                "--control-plane-interval-seconds", "15",
                "--api",
                "--bind", "127.0.0.1",
                "--port", "0"
            ],
            "EnvironmentVariables": [
                "PATH": environment["PATH"] ?? "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
                "HOME": fileManager.homeDirectoryForCurrentUser.path,
                "WC_STORAGE_BACKEND": "local",
                "WC_LOCAL_STORAGE_PATH": storageRoot.path,
                "WC_PROVIDERS": "local",
                "STADO_DEPLOYMENT_ID": deployment.id,
            ],
            "RunAtLoad": true,
            "KeepAlive": ["SuccessfulExit": false],
            "StandardOutPath": logs.appendingPathComponent("service.log").path,
            "StandardErrorPath": logs.appendingPathComponent("service-error.log").path
        ]
        let data = try PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options:
            0)
        try data.write(to: plistURL, options: .atomic)

        await onUpdate(.init(phase: "Starting Stado", detail: "Installing the per-user control-plane service", fraction:
            0.45))
        let errorLog = logs.appendingPathComponent("service-error.log")
        let offset = logOffset(errorLog)
        if !fileManager.fileExists(atPath: errorLog.path) {
            fileManager.createFile(atPath: errorLog.path, contents: nil)
        }
        let domain = "gui/\(getuid())"
        let target = "\(domain)/\(label)"
        do {
            // A unit an earlier install left loaded is booted out first. launchctl
            // print answers 0 for a loaded unit and its service-not-found status
            // for an absent one; any other answer is a failed read, refused here
            // before bootstrap with what launchctl said.
            let probe = try await runStatus("/bin/launchctl", ["print", target])
            switch probe.status {
            case 0:
                try await run("/bin/launchctl", ["bootout", target])
            case Self.launchdServiceNotFound:
                break
            default:
                throw BackendProvisioningError.commandFailed(
                    "launchctl print \(target) exited \(probe.status): \(probe.error)"
                )
            }
            try await retirePerDeploymentUnits(domain: domain, launchAgents: launchAgents)
            // RunAtLoad starts the one process this install reads readiness from.
            try await run("/bin/launchctl", ["bootstrap", domain, plistURL.path])
        } catch {
            throw BackendProvisioningError.commandFailed(error.localizedDescription)
        }

        await onUpdate(.init(phase: "Checking health", detail: "Waiting for the API listener to announce its address", fraction:
            0.75))
        try await awaitLocalServiceReady(target: target, log: errorLog, from: offset)
        guard let endpoint = Self.announcedEndpoint(errorLog, from: offset) else {
            throw BackendProvisioningError.commandFailed(
                "\(target) started but its error log names no API listener address (\(Self.listeningPrefix)…)"
            )
        }
        await onUpdate(.init(phase: "Ready", detail: "This device is running the Stado backend at \(endpoint)", fraction:
            1))
        return ProvisionedBackend(endpoint: endpoint, region: "This Mac")
    }

    /// The one unit Stado runs under, as the Stado catalog names it.
    static let stadoUnit = "com.wisent.stado"

    /// The label prefix earlier releases gave a separate unit per deployment.
    static let retiredDeploymentPrefix = "ai.wisent.stado.deployment."

    /// Boot out and remove every per-deployment unit an earlier release
    /// installed, so com.wisent.stado is the only Stado unit this user runs.
    /// A unit launchd refuses to unload is reported with launchd's words.
    func retirePerDeploymentUnits(domain: String, launchAgents: URL) async throws {
        let names = try fileManager.contentsOfDirectory(atPath: launchAgents.path)
        for name in names where name.hasPrefix(Self.retiredDeploymentPrefix) && name.hasSuffix(".plist") {
            let label = String(name.dropLast(".plist".count))
            let probe = try await runStatus("/bin/launchctl", ["print", "\(domain)/\(label)"])
            switch probe.status {
            case 0:
                try await run("/bin/launchctl", ["bootout", "\(domain)/\(label)"])
            case Self.launchdServiceNotFound:
                break
            default:
                throw BackendProvisioningError.commandFailed(
                    "launchctl print \(domain)/\(label) exited \(probe.status): \(probe.error)"
                )
            }
            try fileManager.removeItem(at: launchAgents.appendingPathComponent(name))
        }
    }
}
