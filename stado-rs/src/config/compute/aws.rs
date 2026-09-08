//! AWS EC2 compute settings.

use std::sync::LazyLock;

use crate::config::resolve_compute_binding;

// AWS uses the same catalog-driven env/config/default precedence as the other
// compute providers. The accessors remain LazyLock-backed because runtime
// configuration is immutable for the process lifetime.
static AWS_REGION: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(crate::capabilities::ProviderId::Aws, "region", "us-east-1")
});
static AWS_SECURITY_GROUP: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(crate::capabilities::ProviderId::Aws, "security-group", "")
});
static AWS_IAM_PROFILE: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Aws,
        "iam-profile",
        "stado-agent",
    )
});
static AWS_AMI_ID: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(crate::capabilities::ProviderId::Aws, "ami-id", "")
        .trim()
        .to_string()
});

/// AWS region for the EC2 provider (env `AWS_REGION`, default us-east-1).
pub fn aws_region() -> &'static str {
    AWS_REGION.as_str()
}

/// AWS security group id for agent instances (env `AWS_SECURITY_GROUP`).
/// Empty means "not configured" — the AWS provider refuses to create.
pub fn aws_security_group() -> &'static str {
    AWS_SECURITY_GROUP.as_str()
}

/// IAM instance profile name attached to agent instances (env
/// `AWS_IAM_PROFILE`, default "stado-agent").
pub fn aws_iam_profile() -> &'static str {
    AWS_IAM_PROFILE.as_str()
}

/// AMI id override (env `AWS_AMI_ID`, whitespace-stripped). Empty falls
/// back to the per-job image argument (Python
/// `os.environ.get("AWS_AMI_ID", "").strip() or image`).
pub fn aws_ami_id() -> &'static str {
    AWS_AMI_ID.as_str()
}
