//! The adapter-owned configuration each runtime variant reads.

use super::field::ConfigField;

const BACKUP_BUCKET_ENV: &str = "WC_BACKUP_BUCKET";
const BACKUP_BUCKET_PATH: &str = "storage.backup.bucket";
const AWS_REGION_CONFIG: ConfigField = ConfigField::scalar("region", "AWS_REGION", "aws.region");

pub(in crate::capabilities) const GCP_COMPUTE_CONFIG: &[ConfigField] = &[
    ConfigField::scalar("project", "GCP_PROJECT", "project").required(),
    ConfigField::scalar("region", "GCP_REGION", "region"),
    ConfigField::list("regions", "GCP_REGIONS", "regions"),
];

pub(in crate::capabilities) const AZURE_COMPUTE_CONFIG: &[ConfigField] = &[
    ConfigField::scalar(
        "subscription-id",
        "AZURE_SUBSCRIPTION_ID",
        "azure.subscription_id",
    )
    .required(),
    ConfigField::scalar(
        "resource-group",
        "AZURE_RESOURCE_GROUP",
        "azure.resource_group",
    ),
    ConfigField::list("locations", "AZURE_LOCATIONS", "azure.locations"),
    ConfigField::scalar("vnet", "AZURE_VNET", "azure.vnet"),
    ConfigField::scalar("subnet", "AZURE_SUBNET", "azure.subnet"),
    ConfigField::scalar("nsg", "AZURE_NSG", "azure.nsg"),
    ConfigField::scalar("image-urn", "AZURE_IMAGE_URN", "azure.image_urn"),
    ConfigField::scalar("vm-username", "AZURE_VM_USERNAME", "azure.vm_username"),
    ConfigField::scalar(
        "vm-identity-id",
        "AZURE_VM_IDENTITY_ID",
        "azure.vm_identity_id",
    )
    .required(),
    ConfigField::scalar(
        "ssh-public-key",
        "AZURE_SSH_PUBLIC_KEY",
        "azure.ssh_public_key",
    )
    .required(),
];

pub(in crate::capabilities) const AWS_COMPUTE_CONFIG: &[ConfigField] = &[
    AWS_REGION_CONFIG,
    ConfigField::scalar("security-group", "AWS_SECURITY_GROUP", "aws.security_group").required(),
    ConfigField::scalar("iam-profile", "AWS_IAM_PROFILE", "aws.iam_profile"),
    ConfigField::scalar("ami-id", "AWS_AMI_ID", "aws.ami_id"),
];

pub(in crate::capabilities) const GCS_CONFIG: &[ConfigField] =
    &[
        ConfigField::scalar("bucket", "WC_BUCKET", "storage.gcs.bucket")
            .required()
            .with_fallback(None, Some("bucket"))
            .with_backup(BACKUP_BUCKET_ENV, BACKUP_BUCKET_PATH, true),
    ];

pub(in crate::capabilities) const AZURE_STORAGE_CONFIG: &[ConfigField] = &[
    ConfigField::scalar(
        "account",
        "WC_AZURE_STORAGE_ACCOUNT",
        "storage.azure.account",
    )
    .required()
    .with_backup(
        "WC_BACKUP_AZURE_STORAGE_ACCOUNT",
        "storage.backup.azure.account",
        true,
    ),
    ConfigField::scalar("container", "WC_AZURE_CONTAINER", "storage.azure.container")
        .required()
        .with_backup(
            "WC_BACKUP_AZURE_CONTAINER",
            "storage.backup.azure.container",
            true,
        ),
];

pub(in crate::capabilities) const S3_CONFIG: &[ConfigField] = &[
    ConfigField::scalar("bucket", "WC_S3_BUCKET", "storage.s3.bucket")
        .required()
        .with_backup(BACKUP_BUCKET_ENV, BACKUP_BUCKET_PATH, true),
    ConfigField::scalar("region", "WC_S3_REGION", "storage.s3.region")
        .with_fallback(Some(AWS_REGION_CONFIG.env), Some(AWS_REGION_CONFIG.path))
        .with_backup("WC_BACKUP_S3_REGION", "storage.backup.s3.region", true),
];

pub(in crate::capabilities) const STADO_OBJECT_STORAGE_CONFIG: &[ConfigField] = &[
    ConfigField::scalar("url", "WC_STADO_STORAGE_URL", "storage.stado.url").required(),
    ConfigField::scalar(
        "token-file",
        "WC_STADO_STORAGE_TOKEN_FILE",
        "storage.stado.token_file",
    )
    .required(),
    ConfigField::scalar(
        "namespace",
        "WC_STADO_STORAGE_NAMESPACE",
        "storage.stado.namespace",
    )
    .required(),
    // Optional on purpose: a loopback HTTP endpoint performs no handshake, and a
    // publicly signed origin is already covered by the system trust store. It is
    // required in exactly one case -- a fleet object API published on the tailnet
    // under a private authority -- and that case previously had no way to say so.
    ConfigField::scalar(
        "ca-file",
        "WC_STADO_STORAGE_CA_FILE",
        "storage.stado.ca_file",
    ),
];

pub(in crate::capabilities) const LOCAL_STORAGE_CONFIG: &[ConfigField] =
    &[
        ConfigField::scalar("path", "WC_LOCAL_STORAGE_PATH", "storage.local.path").with_backup(
            "WC_BACKUP_LOCAL_STORAGE_PATH",
            "storage.backup.local.path",
            true,
        ),
    ];
