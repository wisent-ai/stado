//! `stado host ...` — host health, recovery, user provisioning, and Weles
//! recordings policy, plus read-only diagnostics such as uptime, ping, and
//! exec. Storage inspection and mutation live under the declaration-driven
//! `stado space` capability.
//!
//! One component tree per command family: [`checks`] for health, beacons,
//! recovery and the read-only probes, [`machine`] for users, runners,
//! releases and configuration, [`files`] for delivery, retirement and the
//! local replica, and [`secrets`] for the vault, its grants and the Weles
//! admission surface. Every name this module exposed before the split is
//! re-exported here, so `crate::cli::host::NAME` still resolves.

mod checks;
mod files;
mod machine;
mod secrets;

pub use crate::cli::host::checks::health::health;
pub use crate::cli::host::checks::health::publish::beacon_units;
pub use crate::cli::host::checks::health::publish::collect_beacon;
pub use crate::cli::host::checks::health::publish::publish_beacon;
pub use crate::cli::host::checks::health::units::unit_log;
pub use crate::cli::host::checks::probes::gates::gates;
pub use crate::cli::host::checks::probes::inventory::inventory;
pub use crate::cli::host::checks::probes::vitals::exec;
pub use crate::cli::host::checks::probes::vitals::ping;
pub use crate::cli::host::checks::probes::vitals::uptime;
pub use crate::cli::host::checks::recovery::link::report::link;
pub use crate::cli::host::files::forwarding::deliver;
pub use crate::cli::host::files::remove::remove_file_document;
pub use crate::cli::host::files::remove::remove_run_directory;
pub use crate::cli::host::files::remove::RemoveFileOutcome;
pub use crate::cli::host::files::retire::local::retire_file_local;
pub use crate::cli::host::files::retire::remote::retire_file_outcome;
pub use crate::cli::host::files::retire::RetireFileOutcome;
pub use crate::cli::host::files::retire::RetireFileRequest;
pub use crate::cli::host::files::storage::audit::backup_audit;
pub use crate::cli::host::files::storage::storage_root_reconcile_result;
pub use crate::cli::host::files::storage::storage_root_reconcile_worker;
pub use crate::cli::host::files::storage::StorageRootReconciliationResult;
pub use crate::cli::host::machine::config::config_set;
pub use crate::cli::host::machine::config::config_show;
pub use crate::cli::host::machine::config::config_unset;
pub use crate::cli::host::machine::releases::activate::activate_staged_release;
pub use crate::cli::host::machine::releases::platform::build;
pub use crate::cli::host::machine::releases::platform::run_attached;
pub use crate::cli::host::machine::releases::platform::verify_release_platform;
pub use crate::cli::host::machine::releases::provenance::report::provenance;
pub use crate::cli::host::machine::releases::versions::declare_version;
pub use crate::cli::host::machine::releases::versions::promote::promote_version;
pub use crate::cli::host::machine::users::accounts::user_create;
pub use crate::cli::host::machine::users::accounts::user_delete;
pub use crate::cli::host::machine::users::reboot;
pub use crate::cli::host::machine::users::runners::cron;
pub use crate::cli::host::machine::users::runners::gpu_power_limit;
pub use crate::cli::host::secrets::vault::grants::grant_item_read;
pub use crate::cli::host::secrets::vault::grants::grant_show;
pub use crate::cli::host::secrets::vault::item::put::vault_item_put;
pub use crate::cli::host::secrets::vault::item::retag::retag_vault_item;
pub use crate::cli::host::secrets::vault::item::show::vault_item_show;
pub use crate::cli::host::secrets::vault::mirror::sync::sync_vault;
pub use crate::cli::host::secrets::vault::token::vault_token_mint;
pub use crate::cli::host::secrets::vault::vaults;
pub use crate::cli::host::secrets::weles::sync_acquisition_scopes;
pub use crate::cli::host::secrets::weles::trust::render::render_spis_admission_trust;

pub(crate) use crate::cli::host::checks::health::beacon_store;
pub(crate) use crate::cli::host::checks::recovery::apply_host_repair;
pub(crate) use crate::cli::host::checks::recovery::apply_release_state_repair;
pub(crate) use crate::cli::host::checks::recovery::link::repair::apply_link_repair;
pub(crate) use crate::cli::host::checks::recovery::object_api::apply_agent_skarbiec_repair;
pub(crate) use crate::cli::host::checks::recovery::object_api::apply_object_api_repair;
pub(crate) use crate::cli::host::checks::recovery::object_api::apply_release_store_repair;
pub(crate) use crate::cli::host::checks::recovery::skarbiec::apply_skarbiec_acquisition_repair;
pub(crate) use crate::cli::host::checks::recovery::skarbiec::apply_skarbiec_audit_repair;
pub(crate) use crate::cli::host::checks::recovery::skarbiec::apply_skarbiec_crypto_repair;
pub(crate) use crate::cli::host::checks::recovery::verifier::apply_object_verifier_repair;
pub(crate) use crate::cli::host::checks::recovery::verifier::release::apply_release_verifier_repair;
pub(crate) use crate::cli::host::checks::recovery::verifier::release::apply_service_verifier_repair;
pub(crate) use crate::cli::host::files::forwarding::deliver_file;
pub(crate) use crate::cli::host::files::forwarding::install_secret_value_at_home;
pub(crate) use crate::cli::host::machine::config::remote::remote_config_output;
pub(crate) use crate::cli::host::machine::config::remote::RemoteConfigAction;
pub(crate) use crate::cli::host::machine::users::credentials::credential_host;
pub(crate) use crate::cli::host::machine::users::credentials::release_managed_skarbiec;
