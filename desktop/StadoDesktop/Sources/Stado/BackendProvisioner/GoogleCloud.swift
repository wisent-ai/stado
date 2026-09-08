import Foundation

// MARK: - Google Cloud

extension BackendProvisioner {
    func provisionGCP(
        deployment: StadoDeployment,
        target: InfrastructureTarget,
        onUpdate: UpdateHandler
    ) async throws -> ProvisionedBackend {
        let gcloud = try locateExecutable(named: "gcloud", fixed: [
            "/opt/homebrew/bin/gcloud",
            "/usr/local/bin/gcloud",
            "\(fileManager.homeDirectoryForCurrentUser.path)/google-cloud-sdk/bin/gcloud"
        ])
        let stado = try locateStadoCLI()
        let project = target.externalID
        let region = target.metadata["region"] ?? "us-central1"
        let suffix = deployment.id.lowercased().replacingOccurrences(of: "-", with: "")
        let bucket = "stado-\(suffix)"
        let service = "stado-\(String(suffix.prefix(20)))"
        let serviceAccountName = "stado-\(String(suffix.prefix(12)))"
        let serviceAccount = "\(serviceAccountName)@\(project).iam.gserviceaccount.com"
        let repository = "stado"
        let image = "\(region)-docker.pkg.dev/\(project)/\(repository)/control-plane:\(suffix)"
        let context = try await prepareContainerContext(stadoExecutable: stado)
        defer { try? fileManager.removeItem(at: context) }

        await onUpdate(.init(phase: "Preparing Google Cloud", detail: "Enabling required APIs in \(project)", fraction:
            0.12))
        try await run(gcloud.path, [
            "services", "enable",
            "run.googleapis.com",
            "cloudbuild.googleapis.com",
            "artifactregistry.googleapis.com",
            "compute.googleapis.com",
            "--project", project,
            "--quiet"
        ])

        await onUpdate(.init(phase: "Creating isolated storage", detail: "gs://\(bucket)", fraction:
            0.24))
        if (try? await runCapture(gcloud.path, [
            "storage", "buckets", "describe", "gs://\(bucket)",
            "--project", project
        ])) == nil {
            try await run(gcloud.path, [
                "storage", "buckets", "create", "gs://\(bucket)",
                "--project", project,
                "--location", region,
                "--uniform-bucket-level-access",
                "--quiet"
            ])
        }

        await onUpdate(.init(phase: "Configuring service identity", detail: serviceAccount, fraction:
            0.34))
        if (try? await runCapture(gcloud.path, [
            "iam", "service-accounts", "describe", serviceAccount,
            "--project", project
        ])) == nil {
            try await run(gcloud.path, [
                "iam", "service-accounts", "create", serviceAccountName,
                "--project", project,
                "--display-name", "Stado \(deployment.name)",
                "--quiet"
            ])
        }
        try await run(gcloud.path, [
            "storage", "buckets", "add-iam-policy-binding", "gs://\(bucket)",
            "--member", "serviceAccount:\(serviceAccount)",
            "--role", "roles/storage.objectAdmin",
            "--quiet"
        ])
        for role in [
            "roles/compute.instanceAdmin.v1",
            "roles/iam.serviceAccountUser",
            "roles/serviceusage.serviceUsageConsumer"
        ] {
            try await run(gcloud.path, [
                "projects", "add-iam-policy-binding", project,
                "--member", "serviceAccount:\(serviceAccount)",
                "--role", role,
                "--condition=None",
                "--quiet"
            ])
        }

        if (try? await runCapture(gcloud.path, [
            "artifacts", "repositories", "describe", repository,
            "--project", project,
            "--location", region
        ])) == nil {
            try await run(gcloud.path, [
                "artifacts", "repositories", "create", repository,
                "--repository-format=docker",
                "--location", region,
                "--project", project,
                "--quiet"
            ])
        }

        await onUpdate(.init(phase: "Building Stado", detail: "Cloud Build is packaging the control plane", fraction:
            0.5))
        try await run(gcloud.path, [
            "builds", "submit", context.path,
            "--tag", image,
            "--project", project,
            "--quiet"
        ])

        let environmentFile = context.appendingPathComponent("cloud-run-env.json")
        let environment: [String: String] = [
            "WC_BUCKET": bucket,
            "WC_STORAGE_BACKEND": "gcs",
            "WC_PROVIDERS": "gcp",
            "GCP_PROJECT": project,
            "GCP_REGION": region,
            "STADO_DEPLOYMENT_ID": deployment.id,
            "WC_DASHBOARD_REFRESH_SECONDS": "10"
        ]
        try JSONSerialization.data(withJSONObject: environment, options: [.prettyPrinted])
            .write(to: environmentFile, options: .atomic)

        await onUpdate(.init(phase: "Deploying control plane", detail: "Cloud Run in \(region)", fraction:
            0.72))
        try await run(gcloud.path, [
            "run", "deploy", service,
            "--image", image,
            "--project", project,
            "--region", region,
            "--service-account", serviceAccount,
            "--allow-unauthenticated",
            "--port", "8080",
            "--min", "1",
            "--max", "1",
            "--no-cpu-throttling",
            "--concurrency", "20",
            "--env-vars-file", environmentFile.path,
            "--quiet"
        ])
        let endpoint = try await runCapture(gcloud.path, [
            "run", "services", "describe", service,
            "--project", project,
            "--region", region,
            "--format=value(status.url)"
        ]).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !endpoint.isEmpty else {
            throw BackendProvisioningError.commandFailed("Cloud Run did not return a service URL.")
        }
        await onUpdate(.init(phase: "Checking health", detail: endpoint, fraction:
            0.9))
        try await waitUntilHealthy(endpoint: endpoint)
        await onUpdate(.init(phase: "Ready", detail: "Google Cloud is running this Stado deployment", fraction:
            1))
        return ProvisionedBackend(endpoint: endpoint, region: region)
    }
}
