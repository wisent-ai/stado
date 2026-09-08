import Foundation

// MARK: - Amazon Web Services, network and service

extension BackendProvisioner {
    func provisionAWSService(
        deployment: StadoDeployment,
        aws: URL,
        docker: URL,
        context: URL,
        account: String,
        region: String,
        suffix: String,
        short: String,
        bucket: String,
        repository: String,
        service: String,
        ecrAccessRole: String,
        controlRole: String,
        agentProfile: String,
        writeJSON: AWSArtifactWriter,
        onUpdate: UpdateHandler
    ) async throws -> ProvisionedBackend {
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
            fraction:
                0.48
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
            fraction:
                0.72
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
        await onUpdate(.init(phase: "Checking health", detail: endpoint, fraction:
            0.92))
        try await waitUntilHealthy(endpoint: endpoint)
        await onUpdate(.init(
            phase: "Ready",
            detail: "Amazon Web Services is running this Stado deployment",
            fraction:
                1
        ))
        return ProvisionedBackend(endpoint: endpoint, region: region)
    }
}
