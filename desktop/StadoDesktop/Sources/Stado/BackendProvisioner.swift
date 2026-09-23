import CryptoKit
import Foundation

struct ProvisioningUpdate: Sendable {
    let phase: String
    let detail: String
    let fraction: Double
}

struct ProvisionedBackend: Sendable {
    let endpoint: String
    let region: String?
}

enum BackendProvisioningError: LocalizedError {
    case cliUnavailable
    case unsupportedProvider(String)
    case commandFailed(String)
    case healthCheckFailed(String)

    var errorDescription: String? {
        switch self {
        case .cliUnavailable:
            "Stado CLI is not installed. Install the stado package, then retry."
        case let .unsupportedProvider(provider):
            "Automatic provisioning for \(provider) is not available in this build."
        case let .commandFailed(detail):
            "The control-plane service could not start: \(detail)"
        case let .healthCheckFailed(endpoint):
            "The service started but did not become healthy at \(endpoint)."
        }
    }
}

actor BackendProvisioner {
    typealias UpdateHandler = @Sendable (ProvisioningUpdate) async -> Void

    private let fileManager: FileManager
    private let session: URLSession

    init(fileManager: FileManager = .default, session: URLSession = .shared) {
        self.fileManager = fileManager
        self.session = session
    }

    func provision(
        deployment: StadoDeployment,
        target: InfrastructureTarget,
        onUpdate: UpdateHandler
    ) async throws -> ProvisionedBackend {
        switch target.provider {
        case .local:
            return try await provisionLocal(deployment: deployment, onUpdate: onUpdate)
        case .gcp:
            return try await provisionGCP(deployment: deployment, target: target, onUpdate: onUpdate)
        case .azure:
            return try await provisionAzure(deployment: deployment, target: target, onUpdate: onUpdate)
        case .aws:
            return try await provisionAWS(deployment: deployment, target: target, onUpdate: onUpdate)
        }
    }

    private func provisionGCP(
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

        await onUpdate(.init(phase: "Preparing Google Cloud", detail: "Enabling required APIs in \(project)", fraction: 0.12))
        try await run(gcloud.path, [
            "services", "enable",
            "run.googleapis.com",
            "cloudbuild.googleapis.com",
            "artifactregistry.googleapis.com",
            "compute.googleapis.com",
            "--project", project,
            "--quiet"
        ])

        await onUpdate(.init(phase: "Creating isolated storage", detail: "gs://\(bucket)", fraction: 0.24))
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

        await onUpdate(.init(phase: "Configuring service identity", detail: serviceAccount, fraction: 0.34))
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

        await onUpdate(.init(phase: "Building Stado", detail: "Cloud Build is packaging the control plane", fraction: 0.5))
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

        await onUpdate(.init(phase: "Deploying control plane", detail: "Cloud Run in \(region)", fraction: 0.72))
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
        await onUpdate(.init(phase: "Checking health", detail: endpoint, fraction: 0.9))
        try await waitUntilHealthy(endpoint: endpoint)
        await onUpdate(.init(phase: "Ready", detail: "Google Cloud is running this Stado deployment", fraction: 1))
        return ProvisionedBackend(endpoint: endpoint, region: region)
    }

    private func provisionAzure(
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

        await onUpdate(.init(phase: "Preparing Microsoft Azure", detail: "Selecting subscription \(subscription)", fraction: 0.1))
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

        await onUpdate(.init(phase: "Creating isolated storage", detail: storageAccount, fraction: 0.22))
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

        await onUpdate(.init(phase: "Building Stado", detail: "Azure Container Registry is packaging the control plane", fraction: 0.42))
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

        await onUpdate(.init(phase: "Deploying control plane", detail: "Azure Container Apps in \(region)", fraction: 0.7))
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
        await onUpdate(.init(phase: "Checking health", detail: endpoint, fraction: 0.9))
        try await waitUntilHealthy(endpoint: endpoint)
        await onUpdate(.init(phase: "Ready", detail: "Microsoft Azure is running this Stado deployment", fraction: 1))
        return ProvisionedBackend(endpoint: endpoint, region: region)
    }

    private func provisionAWS(
        deployment: StadoDeployment,
        target: InfrastructureTarget,
        onUpdate: UpdateHandler
    ) async throws -> ProvisionedBackend {
        let aws = try locateExecutable(named: "aws", fixed: [
            "/opt/homebrew/bin/aws",
            "/usr/local/bin/aws"
        ])
        let docker = try locateExecutable(named: "docker", fixed: [
            "/usr/local/bin/docker",
            "/opt/homebrew/bin/docker"
        ])
        let stado = try locateStadoCLI()
        let region = target.metadata["region"] ?? "us-east-1"
        var account = (try? await runCapture(aws.path, [
            "sts", "get-caller-identity", "--query", "Account", "--output", "text"
        ]))?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if account.isEmpty {
            await onUpdate(.init(
                phase: "Signing in to Amazon Web Services",
                detail: "Complete the AWS IAM Identity Center sign-in in your browser",
                fraction: 0.03
            ))
            try await run(aws.path, ["sso", "login"])
            account = try await runCapture(aws.path, [
                "sts", "get-caller-identity", "--query", "Account", "--output", "text"
            ]).trimmingCharacters(in: .whitespacesAndNewlines)
        }
        guard !account.isEmpty else {
            throw BackendProvisioningError.commandFailed("AWS did not return the active account ID.")
        }
        guard target.externalID == account else {
            throw BackendProvisioningError.commandFailed(
                "AWS CLI is signed in to account \(account), but the selected infrastructure target is \(target.externalID)."
            )
        }

        let suffix = deployment.id.lowercased().replacingOccurrences(of: "-", with: "")
        let short = String(suffix.prefix(16))
        let bucket = "stado-\(account)-\(short)"
        let repository = "stado-\(short)"
        let service = "stado-\(short)"
        let ecrAccessRole = "stado-\(short)-ecr"
        let controlRole = "stado-\(short)-control"
        let agentRole = "stado-\(short)-agent"
        let agentProfile = agentRole
        let context = try await prepareContainerContext(stadoExecutable: stado)
        defer { try? fileManager.removeItem(at: context) }

        func writeJSON(_ object: Any, named name: String) throws -> URL {
            let url = context.appendingPathComponent(name)
            try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted])
                .write(to: url, options: .atomic)
            return url
        }

        await onUpdate(.init(
            phase: "Preparing Amazon Web Services",
            detail: "Using account \(account) in \(region)",
            fraction: 0.08
        ))

        await onUpdate(.init(
            phase: "Creating isolated storage",
            detail: "s3://\(bucket)",
            fraction: 0.16
        ))
        if (try? await runCapture(aws.path, [
            "s3api", "head-bucket", "--bucket", bucket, "--region", region
        ])) == nil {
            var arguments = [
                "s3api", "create-bucket", "--bucket", bucket, "--region", region
            ]
            if region != "us-east-1" {
                arguments += [
                    "--create-bucket-configuration",
                    "LocationConstraint=\(region)"
                ]
            }
            try await run(aws.path, arguments)
        }
        let quotas: [String: Any] = [
            "aws": [
                "nvidia-tesla-t4": ["total": 1, "reserved": 0],
                "nvidia-a10": ["total": 1, "reserved": 0],
                "nvidia-l40s": ["total": 1, "reserved": 0],
                "nvidia-a100-80gb": ["total": 1, "reserved": 0],
                "nvidia-h100-80gb": ["total": 1, "reserved": 0]
            ]
        ]
        let quotasURL = try writeJSON(quotas, named: "quotas.json")
        try await run(aws.path, [
            "s3", "cp", quotasURL.path, "s3://\(bucket)/config/quotas.json",
            "--region", region, "--only-show-errors"
        ])

        await onUpdate(.init(
            phase: "Configuring service identities",
            detail: "Creating least-privilege roles for the control plane and GPU workers",
            fraction: 0.27
        ))
        let appRunnerBuildTrust = try writeJSON([
            "Version": "2012-10-17",
            "Statement": [[
                "Effect": "Allow",
                "Principal": ["Service": "build.apprunner.amazonaws.com"],
                "Action": "sts:AssumeRole"
            ]]
        ], named: "apprunner-build-trust.json")
        let appRunnerTaskTrust = try writeJSON([
            "Version": "2012-10-17",
            "Statement": [[
                "Effect": "Allow",
                "Principal": ["Service": "tasks.apprunner.amazonaws.com"],
                "Action": "sts:AssumeRole"
            ]]
        ], named: "apprunner-task-trust.json")
        let ec2Trust = try writeJSON([
            "Version": "2012-10-17",
            "Statement": [[
                "Effect": "Allow",
                "Principal": ["Service": "ec2.amazonaws.com"],
                "Action": "sts:AssumeRole"
            ]]
        ], named: "ec2-trust.json")

        if (try? await runCapture(aws.path, ["iam", "get-role", "--role-name", ecrAccessRole])) == nil {
            try await run(aws.path, [
                "iam", "create-role",
                "--role-name", ecrAccessRole,
                "--assume-role-policy-document", "file://\(appRunnerBuildTrust.path)"
            ])
        }
        try await run(aws.path, [
            "iam", "attach-role-policy",
            "--role-name", ecrAccessRole,
            "--policy-arn", "arn:aws:iam::aws:policy/service-role/AWSAppRunnerServicePolicyForECRAccess"
        ])

        if (try? await runCapture(aws.path, ["iam", "get-role", "--role-name", controlRole])) == nil {
            try await run(aws.path, [
                "iam", "create-role",
                "--role-name", controlRole,
                "--assume-role-policy-document", "file://\(appRunnerTaskTrust.path)"
            ])
        }
        if (try? await runCapture(aws.path, ["iam", "get-role", "--role-name", agentRole])) == nil {
            try await run(aws.path, [
                "iam", "create-role",
                "--role-name", agentRole,
                "--assume-role-policy-document", "file://\(ec2Trust.path)"
            ])
        }

        let bucketARN = "arn:aws:s3:::\(bucket)"
        let agentPolicy = try writeJSON([
            "Version": "2012-10-17",
            "Statement": [[
                "Effect": "Allow",
                "Action": ["s3:ListBucket"],
                "Resource": [bucketARN]
            ], [
                "Effect": "Allow",
                "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
                "Resource": ["\(bucketARN)/*"]
            ]]
        ], named: "agent-policy.json")
        try await run(aws.path, [
            "iam", "put-role-policy",
            "--role-name", agentRole,
            "--policy-name", "StadoQueueAccess",
            "--policy-document", "file://\(agentPolicy.path)"
        ])

        let controlPolicy = try writeJSON([
            "Version": "2012-10-17",
            "Statement": [[
                "Effect": "Allow",
                "Action": ["s3:ListBucket"],
                "Resource": [bucketARN]
            ], [
                "Effect": "Allow",
                "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
                "Resource": ["\(bucketARN)/*"]
            ], [
                "Effect": "Allow",
                "Action": [
                    "ec2:DescribeInstances", "ec2:DescribeSubnets",
                    "ec2:DescribeSecurityGroups", "ec2:RunInstances",
                    "ec2:CreateTags", "ec2:TerminateInstances"
                ],
                "Resource": ["*"]
            ], [
                "Effect": "Allow",
                "Action": ["iam:PassRole"],
                "Resource": ["arn:aws:iam::\(account):role/\(agentRole)"]
            ]]
        ], named: "control-policy.json")
        try await run(aws.path, [
            "iam", "put-role-policy",
            "--role-name", controlRole,
            "--policy-name", "StadoControlPlane",
            "--policy-document", "file://\(controlPolicy.path)"
        ])

        if (try? await runCapture(aws.path, [
            "iam", "get-instance-profile", "--instance-profile-name", agentProfile
        ])) == nil {
            try await run(aws.path, [
                "iam", "create-instance-profile",
                "--instance-profile-name", agentProfile
            ])
        }
        let attachedAgentRole = (try? await runCapture(aws.path, [
            "iam", "get-instance-profile",
            "--instance-profile-name", agentProfile,
            "--query", "InstanceProfile.Roles[?RoleName=='\(agentRole)'].RoleName | [0]",
            "--output", "text"
        ]))?.trimmingCharacters(in: .whitespacesAndNewlines)
        if attachedAgentRole != agentRole {
            try await run(aws.path, [
                "iam", "add-role-to-instance-profile",
                "--instance-profile-name", agentProfile,
                "--role-name", agentRole
            ])
        }

        let defaultVPC = try await runCapture(aws.path, [
            "ec2", "describe-vpcs",
            "--filters", "Name=is-default,Values=true",
            "--query", "Vpcs[0].VpcId", "--output", "text",
            "--region", region
        ]).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !defaultVPC.isEmpty, defaultVPC != "None" else {
            throw BackendProvisioningError.commandFailed(
                "AWS account has no default VPC in \(region). Create one, then retry."
            )
        }
        let securityGroupName = "stado-\(short)-agents"
        var securityGroup = (try? await runCapture(aws.path, [
            "ec2", "describe-security-groups",
            "--filters",
            "Name=group-name,Values=\(securityGroupName)",
            "Name=vpc-id,Values=\(defaultVPC)",
            "--query", "SecurityGroups[0].GroupId", "--output", "text",
            "--region", region
        ]))?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if securityGroup.isEmpty || securityGroup == "None" {
            securityGroup = try await runCapture(aws.path, [
                "ec2", "create-security-group",
                "--group-name", securityGroupName,
                "--description", "Outbound-only Stado GPU workers",
                "--vpc-id", defaultVPC,
                "--query", "GroupId", "--output", "text",
                "--region", region
            ]).trimmingCharacters(in: .whitespacesAndNewlines)
        }

        let ami = try await runCapture(aws.path, [
            "ssm", "get-parameter",
            "--name", "/aws/service/deeplearning/ami/x86_64/base-oss-nvidia-driver-gpu-ubuntu-22.04/latest/ami-id",
            "--query", "Parameter.Value", "--output", "text",
            "--region", region
        ]).trimmingCharacters(in: .whitespacesAndNewlines)
        guard ami.hasPrefix("ami-") else {
            throw BackendProvisioningError.commandFailed(
                "AWS did not return the current NVIDIA Deep Learning AMI."
            )
        }

        await onUpdate(.init(
            phase: "Building Stado",
            detail: "Publishing the control plane to Amazon ECR",
            fraction: 0.48
        ))
        if (try? await runCapture(aws.path, [
            "ecr", "describe-repositories",
            "--repository-names", repository,
            "--region", region
        ])) == nil {
            try await run(aws.path, [
                "ecr", "create-repository",
                "--repository-name", repository,
                "--image-scanning-configuration", "scanOnPush=true",
                "--region", region
            ])
        }
        let registry = "\(account).dkr.ecr.\(region).amazonaws.com"
        let image = "\(registry)/\(repository):\(suffix)"
        let password = try await runCapture(aws.path, [
            "ecr", "get-login-password", "--region", region
        ])
        try await runWithInput(
            docker.path,
            ["login", "--username", "AWS", "--password-stdin", registry],
            input: password
        )
        try await run(docker.path, [
            "build", "--platform", "linux/amd64", "--tag", image, context.path
        ])
        try await run(docker.path, ["push", image])

        let environment: [String: String] = [
            "WC_BUCKET": bucket,
            "WC_STORAGE_BACKEND": "s3",
            "WC_S3_BUCKET": bucket,
            "WC_S3_REGION": region,
            "WC_PROVIDERS": "aws",
            "AWS_REGION": region,
            "AWS_SECURITY_GROUP": securityGroup,
            "AWS_IAM_PROFILE": agentProfile,
            "AWS_AMI_ID": ami,
            "STADO_DEPLOYMENT_ID": deployment.id,
            "WC_DASHBOARD_REFRESH_SECONDS": "10"
        ]
        let sourceConfiguration = try writeJSON([
            "AuthenticationConfiguration": [
                "AccessRoleArn": "arn:aws:iam::\(account):role/\(ecrAccessRole)"
            ],
            "AutoDeploymentsEnabled": false,
            "ImageRepository": [
                "ImageIdentifier": image,
                "ImageRepositoryType": "ECR",
                "ImageConfiguration": [
                    "Port": "8080",
                    "RuntimeEnvironmentVariables": environment
                ]
            ]
        ], named: "apprunner-source.json")
        let instanceConfiguration = try writeJSON([
            "Cpu": "1 vCPU",
            "Memory": "2 GB",
            "InstanceRoleArn": "arn:aws:iam::\(account):role/\(controlRole)"
        ], named: "apprunner-instance.json")

        await onUpdate(.init(
            phase: "Deploying control plane",
            detail: "AWS App Runner in \(region)",
            fraction: 0.72
        ))
        var serviceARN = (try? await runCapture(aws.path, [
            "apprunner", "list-services",
            "--query", "ServiceSummaryList[?ServiceName=='\(service)'].ServiceArn | [0]",
            "--output", "text", "--region", region
        ]))?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if serviceARN.isEmpty || serviceARN == "None" {
            serviceARN = try await runCapture(aws.path, [
                "apprunner", "create-service",
                "--service-name", service,
                "--source-configuration", "file://\(sourceConfiguration.path)",
                "--instance-configuration", "file://\(instanceConfiguration.path)",
                "--health-check-configuration",
                "Protocol=HTTP,Path=/healthz,Interval=10,Timeout=5,HealthyThreshold=1,UnhealthyThreshold=5",
                "--query", "Service.ServiceArn", "--output", "text",
                "--region", region
            ]).trimmingCharacters(in: .whitespacesAndNewlines)
        } else {
            try await run(aws.path, [
                "apprunner", "update-service",
                "--service-arn", serviceARN,
                "--source-configuration", "file://\(sourceConfiguration.path)",
                "--instance-configuration", "file://\(instanceConfiguration.path)",
                "--region", region
            ])
        }
        try await run(aws.path, [
            "apprunner", "wait", "service-running",
            "--service-arn", serviceARN,
            "--region", region
        ])
        let hostname = try await runCapture(aws.path, [
            "apprunner", "describe-service",
            "--service-arn", serviceARN,
            "--query", "Service.ServiceUrl", "--output", "text",
            "--region", region
        ]).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !hostname.isEmpty, hostname != "None" else {
            throw BackendProvisioningError.commandFailed("AWS App Runner did not return a service URL.")
        }
        let endpoint = "https://\(hostname)"
        await onUpdate(.init(phase: "Checking health", detail: endpoint, fraction: 0.92))
        try await waitUntilHealthy(endpoint: endpoint)
        await onUpdate(.init(
            phase: "Ready",
            detail: "Amazon Web Services is running this Stado deployment",
            fraction: 1
        ))
        return ProvisionedBackend(endpoint: endpoint, region: region)
    }

    private func provisionLocal(
        deployment: StadoDeployment,
        onUpdate: UpdateHandler
    ) async throws -> ProvisionedBackend {
        await onUpdate(.init(phase: "Preparing this device", detail: "Creating an isolated deployment directory", fraction: 0.15))
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
            "ThrottleInterval": 5,
            "StandardOutPath": logs.appendingPathComponent("service.log").path,
            "StandardErrorPath": logs.appendingPathComponent("service-error.log").path
        ]
        let data = try PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options: 0)
        try data.write(to: plistURL, options: .atomic)

        await onUpdate(.init(phase: "Starting Stado", detail: "Installing the per-user control-plane service", fraction: 0.45))
        let domain = "gui/\(getuid())"
        _ = try? await run("/bin/launchctl", ["bootout", "\(domain)/\(label)"])
        do {
            try await run("/bin/launchctl", ["bootstrap", domain, plistURL.path])
            try await run("/bin/launchctl", ["kickstart", "-k", "\(domain)/\(label)"])
        } catch {
            throw BackendProvisioningError.commandFailed(error.localizedDescription)
        }

        await onUpdate(.init(phase: "Checking health", detail: endpoint, fraction: 0.75))
        try await waitUntilHealthy(endpoint: endpoint)
        await onUpdate(.init(phase: "Ready", detail: "This device is running the Stado backend", fraction: 1))
        return ProvisionedBackend(endpoint: endpoint, region: "This Mac")
    }

    private func prepareContainerContext(stadoExecutable: URL) async throws -> URL {
        let rootValue = try await runCapture(stadoExecutable.path, ["package-root"])
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let source = URL(fileURLWithPath: rootValue).appendingPathComponent("stado", isDirectory: true)
        guard fileManager.fileExists(atPath: source.appendingPathComponent("cli.py").path) else {
            throw BackendProvisioningError.commandFailed("The installed Stado package source could not be located.")
        }
        let context = fileManager.temporaryDirectory
            .appendingPathComponent("stado-cloud-\(UUID().uuidString)", isDirectory: true)
        try fileManager.createDirectory(at: context, withIntermediateDirectories: true)
        try fileManager.copyItem(at: source, to: context.appendingPathComponent("stado", isDirectory: true))
        let dockerfile = """
        FROM python:3.12-slim
        ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 PYTHONPATH=/app
        WORKDIR /app
        RUN pip install --no-cache-dir 'stado[aws,azure]'
        COPY stado /app/stado
        CMD ["stado", "cloud-control-plane", "--bind", "0.0.0.0", "--port", "8080", "--interval", "30"]
        """
        try dockerfile.write(
            to: context.appendingPathComponent("Dockerfile"),
            atomically: true,
            encoding: .utf8
        )
        return context
    }

    private func locateExecutable(named name: String, fixed: [String]) throws -> URL {
        let environment = ProcessInfo.processInfo.environment
        let candidates = (environment["PATH"] ?? "")
            .split(separator: ":")
            .map(String.init)
            .map { URL(fileURLWithPath: $0).appendingPathComponent(name) }
            + fixed.map { URL(fileURLWithPath: $0) }
        guard let executable = candidates.first(where: {
            fileManager.isExecutableFile(atPath: $0.path)
        }) else {
            throw BackendProvisioningError.commandFailed("\(name) is not installed or is not executable.")
        }
        return executable
    }

    private func locateStadoCLI() throws -> URL {
        let environment = ProcessInfo.processInfo.environment
        let pathCandidates = (environment["PATH"] ?? "")
            .split(separator: ":")
            .map(String.init)
            .map { URL(fileURLWithPath: $0).appendingPathComponent("stado") }
        let fixedCandidates = [
            fileManager.homeDirectoryForCurrentUser.appendingPathComponent(".local/bin/stado"),
            URL(fileURLWithPath: "/opt/homebrew/bin/stado"),
            URL(fileURLWithPath: "/usr/local/bin/stado"),
            URL(fileURLWithPath: "/opt/homebrew/Caskroom/miniforge/base/bin/stado"),
        ]
        guard let executable = (pathCandidates + fixedCandidates).first(where: {
            fileManager.isExecutableFile(atPath: $0.path)
        }) else {
            throw BackendProvisioningError.cliUnavailable
        }
        return executable
    }

    private func stablePort(for deploymentID: String) -> Int {
        let digest = SHA256.hash(data: Data(deploymentID.utf8))
        let value = digest.prefix(2).reduce(0) { ($0 << 8) | Int($1) }
        return 8800 + value % 1000
    }

    private func waitUntilHealthy(endpoint: String) async throws {
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

    private func run(_ executable: String, _ arguments: [String]) async throws {
        try await Task.detached(priority: .utility) {
            let process = Process()
            let errors = Pipe()
            process.executableURL = URL(fileURLWithPath: executable)
            process.arguments = arguments
            process.standardOutput = FileHandle.nullDevice
            process.standardError = errors
            try process.run()
            let errorData = errors.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            guard process.terminationStatus == 0 else {
                let detail = String(data: errorData, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines)
                throw BackendProvisioningError.commandFailed(detail?.isEmpty == false ? detail! : "exit \(process.terminationStatus)")
            }
        }.value
    }

    private func runWithInput(
        _ executable: String,
        _ arguments: [String],
        input: String
    ) async throws {
        try await Task.detached(priority: .utility) {
            let process = Process()
            let standardInput = Pipe()
            let errors = Pipe()
            process.executableURL = URL(fileURLWithPath: executable)
            process.arguments = arguments
            process.standardInput = standardInput
            process.standardOutput = FileHandle.nullDevice
            process.standardError = errors
            try process.run()
            standardInput.fileHandleForWriting.write(Data(input.utf8))
            try standardInput.fileHandleForWriting.close()
            let errorData = errors.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            guard process.terminationStatus == 0 else {
                let detail = String(data: errorData, encoding: .utf8)?
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                throw BackendProvisioningError.commandFailed(
                    detail?.isEmpty == false ? detail! : "exit \(process.terminationStatus)"
                )
            }
        }.value
    }

    private func runCapture(_ executable: String, _ arguments: [String]) async throws -> String {
        try await Task.detached(priority: .utility) {
            let process = Process()
            let output = Pipe()
            process.executableURL = URL(fileURLWithPath: executable)
            process.arguments = arguments
            process.standardOutput = output
            process.standardError = output
            try process.run()
            let data = output.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            let text = String(data: data, encoding: .utf8) ?? ""
            guard process.terminationStatus == 0 else {
                let detail = text.trimmingCharacters(in: .whitespacesAndNewlines)
                throw BackendProvisioningError.commandFailed(
                    detail.isEmpty ? "exit \(process.terminationStatus)" : detail
                )
            }
            return text
        }.value
    }
}
