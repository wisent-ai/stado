import Foundation

// MARK: - Microsoft Azure

extension BackendProvisioner {
    func provisionAzure(
        deployment: StadoDeployment,
        target: InfrastructureTarget,
        onUpdate: UpdateHandler
    ) async throws -> ProvisionedBackend {
        let az = try locateExecutable(named: "az", fixed: [
            "/opt/homebrew/bin/az",
            "/usr/local/bin/az"
        ])
        let stado = try locateStadoCLI()
        let subscription = target.externalID
        let region = target.metadata["location"] ?? "eastus"
        let suffix = deployment.id.lowercased().replacingOccurrences(of: "-", with: "")
        let short = String(suffix.prefix(16))
        let resourceGroup = "stado-\(short)-rg"
        let storageAccount = "stado\(String(suffix.prefix(19)))"
        let registry = "stado\(String(suffix.suffix(19)))"
        let environmentName = "stado-\(short)-env"
        let appName = "stado-\(short)"
        let image = "\(registry).azurecr.io/control-plane:\(suffix)"
        let context = try await prepareContainerContext(stadoExecutable: stado)
        defer { try? fileManager.removeItem(at: context) }

        await onUpdate(.init(phase: "Preparing Microsoft Azure", detail: "Selecting subscription \(subscription)", fraction:
            0.1))
        try await run(az.path, ["account", "set", "--subscription", subscription])
        _ = try? await run(az.path, ["extension", "add", "--name", "containerapp", "--upgrade", "--yes"])
        try await run(az.path, [
            "provider", "register",
            "--namespace", "Microsoft.App",
            "--wait"
        ])
        try await run(az.path, [
            "group", "create",
            "--name", resourceGroup,
            "--location", region,
            "--output", "none"
        ])

        await onUpdate(.init(phase: "Creating isolated storage", detail: storageAccount, fraction:
            0.22))
        if (try? await runCapture(az.path, [
            "storage", "account", "show",
            "--name", storageAccount,
            "--resource-group", resourceGroup,
            "--output", "none"
        ])) == nil {
            try await run(az.path, [
                "storage", "account", "create",
                "--name", storageAccount,
                "--resource-group", resourceGroup,
                "--location", region,
                "--sku", "Standard_LRS",
                "--kind", "StorageV2",
                "--allow-blob-public-access", "false",
                "--output", "none"
            ])
        }
        try await run(az.path, [
            "storage", "container", "create",
            "--name", "stado",
            "--account-name", storageAccount,
            "--auth-mode", "login",
            "--output", "none"
        ])

        await onUpdate(.init(phase: "Building Stado", detail: "Azure Container Registry is packaging the control plane", fraction:
            0.42))
        if (try? await runCapture(az.path, [
            "acr", "show",
            "--name", registry,
            "--resource-group", resourceGroup,
            "--output", "none"
        ])) == nil {
            try await run(az.path, [
                "acr", "create",
                "--name", registry,
                "--resource-group", resourceGroup,
                "--sku", "Basic",
                "--admin-enabled", "false",
                "--output", "none"
            ])
        }
        try await run(az.path, [
            "acr", "build",
            "--registry", registry,
            "--image", "control-plane:\(suffix)",
            "--file", context.appendingPathComponent("Dockerfile").path,
            context.path,
            "--output", "none"
        ])

        if (try? await runCapture(az.path, [
            "containerapp", "env", "show",
            "--name", environmentName,
            "--resource-group", resourceGroup,
            "--output", "none"
        ])) == nil {
            try await run(az.path, [
                "containerapp", "env", "create",
                "--name", environmentName,
                "--resource-group", resourceGroup,
                "--location", region,
                "--output", "none"
            ])
        }

        let environmentValues = [
            "WC_BUCKET=stado",
            "WC_STORAGE_BACKEND=azure",
            "WC_AZURE_STORAGE_ACCOUNT=\(storageAccount)",
            "WC_AZURE_CONTAINER=stado",
            "WC_PROVIDERS=azure",
            "AZURE_SUBSCRIPTION_ID=\(subscription)",
            "AZURE_RESOURCE_GROUP=\(resourceGroup)",
            "AZURE_REGION=\(region)",
            "STADO_DEPLOYMENT_ID=\(deployment.id)",
            "WC_DASHBOARD_REFRESH_SECONDS=10"
        ]

        await onUpdate(.init(phase: "Deploying control plane", detail: "Azure Container Apps in \(region)", fraction:
            0.7))
        if (try? await runCapture(az.path, [
            "containerapp", "show",
            "--name", appName,
            "--resource-group", resourceGroup,
            "--output", "none"
        ])) == nil {
            try await run(az.path, [
                "containerapp", "create",
                "--name", appName,
                "--resource-group", resourceGroup,
                "--environment", environmentName,
                "--image", image,
                "--system-assigned",
                "--registry-server", "\(registry).azurecr.io",
                "--registry-identity", "system",
                "--ingress", "external",
                "--target-port", "8080",
                "--transport", "http",
                "--min-replicas", "1",
                "--max-replicas", "1",
                "--cpu", "1.0",
                "--memory", "2Gi",
                "--env-vars"
            ] + environmentValues + ["--output", "none"])
        } else {
            try await run(az.path, [
                "containerapp", "update",
                "--name", appName,
                "--resource-group", resourceGroup,
                "--image", image,
                "--min-replicas", "1",
                "--max-replicas", "1",
                "--set-env-vars"
            ] + environmentValues + ["--output", "none"])
        }

        let principalID = try await runCapture(az.path, [
            "containerapp", "identity", "show",
            "--name", appName,
            "--resource-group", resourceGroup,
            "--query", "principalId",
            "--output", "tsv"
        ]).trimmingCharacters(in: .whitespacesAndNewlines)
        let storageScope = try await runCapture(az.path, [
            "storage", "account", "show",
            "--name", storageAccount,
            "--resource-group", resourceGroup,
            "--query", "id",
            "--output", "tsv"
        ]).trimmingCharacters(in: .whitespacesAndNewlines)
        let groupScope = try await runCapture(az.path, [
            "group", "show",
            "--name", resourceGroup,
            "--query", "id",
            "--output", "tsv"
        ]).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !principalID.isEmpty, !storageScope.isEmpty, !groupScope.isEmpty else {
            throw BackendProvisioningError.commandFailed("Azure did not return the managed identity or resource scopes.")
        }
        try await run(az.path, [
            "role", "assignment", "create",
            "--assignee-object-id", principalID,
            "--assignee-principal-type", "ServicePrincipal",
            "--role", "Storage Blob Data Contributor",
            "--scope", storageScope,
            "--output", "none"
        ])
        for role in ["Contributor", "User Access Administrator"] {
            try await run(az.path, [
                "role", "assignment", "create",
                "--assignee-object-id", principalID,
                "--assignee-principal-type", "ServicePrincipal",
                "--role", role,
                "--scope", groupScope,
                "--output", "none"
            ])
        }
        try await run(az.path, [
            "containerapp", "revision", "restart",
            "--name", appName,
            "--resource-group", resourceGroup,
            "--revision",
            try await runCapture(az.path, [
                "containerapp", "revision", "list",
                "--name", appName,
                "--resource-group", resourceGroup,
                "--query", "[0].name",
                "--output", "tsv"
            ]).trimmingCharacters(in: .whitespacesAndNewlines)
        ])
        let fqdn = try await runCapture(az.path, [
            "containerapp", "show",
            "--name", appName,
            "--resource-group", resourceGroup,
            "--query", "properties.configuration.ingress.fqdn",
            "--output", "tsv"
        ]).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !fqdn.isEmpty else {
            throw BackendProvisioningError.commandFailed("Azure did not return the Container App hostname.")
        }
        let endpoint = "https://\(fqdn)"
        await onUpdate(.init(phase: "Checking health", detail: endpoint, fraction:
            0.9))
        try await waitUntilHealthy(endpoint: endpoint)
        await onUpdate(.init(phase: "Ready", detail: "Microsoft Azure is running this Stado deployment", fraction:
            1))
        return ProvisionedBackend(endpoint: endpoint, region: region)
    }
}
