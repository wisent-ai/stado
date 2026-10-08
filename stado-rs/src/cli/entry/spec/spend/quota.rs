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
    /// Quota-increase requests: submit them, and list the ones in flight.
    Request {
        #[command(subcommand)]
        command: QuotaRequestCommands,
    },
    /// The provider support tickets quota requests turn into.
    Ticket {
        #[command(subcommand)]
        command: QuotaTicketCommands,
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
}

#[derive(Subcommand)]
pub(crate) enum QuotaRequestCommands {
    /// Submit GPU quota-increase requests through the provider Quotas API:
    /// for ACCEL, or with --every-family for every GPU family the catalog
    /// reports, one request per provider and region.
    Create {
        /// The accelerator to raise the limit of.
        #[arg(
            required_unless_present = "every_family",
            conflicts_with = "every_family"
        )]
        accel: Option<String>,
        /// Request every GPU family the provider's catalog reports.
        #[arg(long)]
        every_family: bool,
        /// New per-region quota limit to request.
        #[arg(long = "to", required = true)]
        new_limit: i64,
        /// Comma-separated regions/locations; default = every region
        /// the provider dispatches into (REGIONS / AZURE_LOCATIONS).
        #[arg(long, default_value = "")]
        region: String,
        /// Comma-separated provider list (gcp,azure); default = WC_PROVIDERS.
        #[arg(long, default_value = "")]
        provider: String,
        /// The reason the provider's reviewer reads, stated by the operator.
        #[arg(long)]
        justification: String,
        /// Contact email for the Cloud Quotas reviewer (required for GCP).
        /// Default: $WC_QUOTA_CONTACT_EMAIL.
        #[arg(long, default_value = "")]
        email: String,
        /// Emit machine-readable JSON result list.
        #[arg(long)]
        json: bool,
    },
    /// Cross-provider in-flight quota requests + support communications.
    List {
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

#[derive(Subcommand)]
pub(crate) enum QuotaTicketCommands {
    /// Respond to open quota support tickets the provider is waiting on us for.
    Reply {
        /// The provider whose support tickets are answered.
        #[arg(long, value_enum)]
        provider: TicketProvider,
        /// Print what would be sent without posting it.
        #[arg(long)]
        dry_run: bool,
        /// Contact email shown in the response signature.
        /// Default: $WC_QUOTA_CONTACT_EMAIL.
        #[arg(long, default_value = "")]
        email: String,
    },
    /// Post a credit-funded-subscription escalation reply on every open
    /// quota ticket whose latest provider message was a billing-side denial.
    Escalate {
        /// The provider whose support tickets are escalated.
        #[arg(long, value_enum)]
        provider: TicketProvider,
        /// Print what would be sent without posting it.
        #[arg(long)]
        dry_run: bool,
        /// Contact email shown in the response signature.
        /// Default: $WC_QUOTA_CONTACT_EMAIL.
        #[arg(long, default_value = "")]
        email: String,
    },
}

/// Providers whose quota requests become support tickets Stado can answer.
/// A provider without that adapter is refused by clap with this list.
#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum TicketProvider {
    Azure,
}
