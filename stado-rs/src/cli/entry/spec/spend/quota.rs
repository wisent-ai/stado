//! Provider GPU quota: what we hold, what we have asked for, and the support
//! conversations those requests turn into.

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum QuotaCommands {
    /// Show GPU quota totals across all providers in WC_PROVIDERS.
    Show {
        /// Emit machine-readable JSON instead of the table.
        #[arg(long)]
        json: bool,
    },
    /// Submit GPU quota-increase request(s) for ACCEL via the provider Quotas API.
    Request {
        accel: String,
        /// New per-region quota limit to request (e.g. 16).
        #[arg(long = "to", required = true)]
        new_limit: i64,
        /// Comma-separated regions/locations; default = every region
        /// the provider dispatches into (REGIONS / AZURE_LOCATIONS).
        #[arg(long, default_value = "")]
        region: String,
        /// Comma-separated provider list (gcp,azure); default = WC_PROVIDERS.
        #[arg(long, default_value = "")]
        provider: String,
        /// Reviewer-visible justification text.
        #[arg(
            long,
            default_value = "wisent-compute autoscaler queue depth requires more parallel GPU capacity"
        )]
        justification: String,
        /// Contact email for the Cloud Quotas reviewer (required for GCP).
        /// Default: $WC_QUOTA_CONTACT_EMAIL.
        #[arg(long, default_value = "")]
        email: String,
        /// Emit machine-readable JSON result list.
        #[arg(long)]
        json: bool,
    },
    /// Respond to Open Azure quota support tickets awaiting customer info.
    #[command(name = "azure-replies")]
    AzureReplies {
        /// Print what would be sent without invoking az
        /// support communication create.
        #[arg(long)]
        dry_run: bool,
        /// Contact email shown in the response signature.
        /// Default: $WC_QUOTA_CONTACT_EMAIL.
        #[arg(long, default_value = "")]
        email: String,
    },
    /// Post a credit-funded-subscription escalation reply on every
    /// Open Azure quota ticket whose latest Microsoft message was a
    /// billing-side denial.
    #[command(name = "azure-escalate")]
    AzureEscalate {
        /// Print what would be sent without invoking az
        /// support communication create.
        #[arg(long)]
        dry_run: bool,
        /// Contact email shown in the response signature.
        /// Default: $WC_QUOTA_CONTACT_EMAIL.
        #[arg(long, default_value = "")]
        email: String,
    },
    /// List the full GPU catalog for each provider in WC_PROVIDERS.
    Catalog {
        /// Comma-separated provider list (gcp,azure); default = WC_PROVIDERS.
        #[arg(long, default_value = "")]
        provider: String,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Submit quota-increase requests for EVERY known GPU family on each provider.
    #[command(name = "request-all")]
    RequestAll {
        /// New per-region quota limit to request for every GPU family.
        #[arg(long = "to", required = true)]
        new_limit: i64,
        /// Comma-separated provider list (gcp,azure); default = WC_PROVIDERS.
        #[arg(long, default_value = "")]
        provider: String,
        /// Comma-separated regions/locations; default = the provider's
        /// configured REGIONS / AZURE_LOCATIONS.
        #[arg(long, default_value = "")]
        region: String,
        /// Reviewer-visible justification text.
        #[arg(
            long,
            default_value = "wisent-compute autoscaler bulk capacity request: provision GPU headroom across every supported family in the dispatch regions so the scheduler can use whichever family Google/Azure can serve."
        )]
        justification: String,
        /// Contact email for the GCP Cloud Quotas reviewer.
        /// Default: $WC_QUOTA_CONTACT_EMAIL.
        #[arg(long, default_value = "")]
        email: String,
        /// Emit machine-readable JSON result list.
        #[arg(long)]
        json: bool,
    },
    /// Cross-provider in-flight quota requests + support communications.
    Requests {
        /// Comma-separated provider list (gcp,azure); default = WC_PROVIDERS.
        #[arg(long, default_value = "")]
        provider: String,
        /// Filter GCP rows by state (reconciling, approved, denied,
        /// partially_approved, unknown); empty = all.
        #[arg(long, default_value = "")]
        state: String,
        /// For Azure, only show tickets where Microsoft has the
        /// latest message and is awaiting a customer reply.
        #[arg(long)]
        awaiting_customer: bool,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
}
