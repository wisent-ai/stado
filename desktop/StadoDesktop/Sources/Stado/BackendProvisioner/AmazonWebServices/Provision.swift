import Foundation

// MARK: - Amazon Web Services, account and identities

extension BackendProvisioner {
    func provisionAWS(
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
                fraction:
                    0.03
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

        let writeJSON = AWSArtifactWriter(directory: context)

        await onUpdate(.init(
            phase: "Preparing Amazon Web Services",
            detail: "Using account \(account) in \(region)",
            fraction:
                0.08
        ))

        await onUpdate(.init(
            phase: "Creating isolated storage",
            detail: "s3://\(bucket)",
            fraction:
                0.16
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
                "nvidia-tesla-t4": ["total":
                    1, "reserved":
                    0],
                "nvidia-a10": ["total":
                    1, "reserved":
                    0],
                "nvidia-l40s": ["total":
                    1, "reserved":
                    0],
                "nvidia-a100-80gb": ["total":
                    1, "reserved":
                    0],
                "nvidia-h100-80gb": ["total":
                    1, "reserved":
                    0]
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
            fraction:
                0.27
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

        return try await provisionAWSService(
            deployment: deployment,
            aws: aws,
            docker: docker,
            context: context,
            account: account,
            region: region,
            suffix: suffix,
            short: short,
            bucket: bucket,
            repository: repository,
            service: service,
            ecrAccessRole: ecrAccessRole,
            controlRole: controlRole,
            agentProfile: agentProfile,
            writeJSON: writeJSON,
            onUpdate: onUpdate
        )
    }
}
