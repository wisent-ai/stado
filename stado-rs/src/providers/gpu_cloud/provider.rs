//! The one `Provider` implementation every GPU cloud vendor shares.
//!
//! An instance reference is `<guest hostname>@<native id>`: the part before
//! `@` is the name the agent publishes capacity under and the reaper matches,
//! the part after it is what the vendor's API takes back. Only machines whose
//! name carries the fleet's instance prefix are agents; anything else the
//! credential can see is left alone.

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::Utc;

use crate::capabilities::GpuCloudVendor;
use crate::providers::{Provider, ProviderError};

use super::api::{GpuCloudApi, GpuCloudError, LaunchRequest, Machine};
use super::{api, profile, GuestIdentity};

/// A GPU cloud vendor behind Stado's provider contract.
pub struct GpuCloudProvider {
    vendor: GpuCloudVendor,
    api: Box<dyn GpuCloudApi>,
}

impl GpuCloudProvider {
    pub fn new(vendor: GpuCloudVendor) -> Self {
        Self {
            vendor,
            api: api(vendor),
        }
    }

    fn log(&self, message: &str) {
        eprintln!("[{}] {message}", self.vendor.provider().as_str());
    }

    /// The reference Stado keeps for `machine`.
    fn reference(&self, machine: &Machine) -> String {
        let worker = match profile(self.vendor).guest {
            // A machine no agent boots on is still listed, read and released
            // under the name Stado gave it at launch.
            GuestIdentity::InstanceName
            | GuestIdentity::ContainerName
            | GuestIdentity::NoStartupScript => machine.name.as_str(),
            GuestIdentity::ContainerEnv(_) => machine.native_id.as_str(),
        };
        format!("{worker}@{}", machine.native_id)
    }

    /// The vendor's id inside a reference Stado handed out.
    fn native_id<'a>(&self, instance_ref: &'a str) -> Result<&'a str, ProviderError> {
        instance_ref
            .split_once('@')
            .map(|(_, native)| native)
            .filter(|native| !native.is_empty())
            .ok_or_else(|| {
                ProviderError::Value(format!(
                    "{}: instance reference {instance_ref:?} is not <hostname>@<{} id>",
                    self.vendor.display_name(),
                    self.vendor.display_name()
                ))
            })
    }

    /// Agent machines: the fleet's naming prefix, alive.
    async fn live_agents(&self) -> Result<Vec<Machine>, ProviderError> {
        Ok(self
            .api
            .machines()
            .await?
            .into_iter()
            .filter(|machine| {
                machine.name.starts_with(crate::config::INSTANCE_PREFIX) && machine.state.alive()
            })
            .collect())
    }

    /// The accelerator a vendor instance type carries, from the vendor's
    /// declared offers; the type itself when it is not one of them.
    fn accel_of(&self, instance_type: &str) -> String {
        profile(self.vendor)
            .offers
            .iter()
            .find(|(offered, _)| *offered == instance_type)
            .map_or_else(|| instance_type.to_string(), |(_, accel)| accel.to_string())
    }
}

/// The startup script with one line after the interpreter line that gives
/// the agent its worker name before it asks for one: a VM sets its kernel
/// hostname, a container exports `STADO_WORKER_NAME`.
fn with_identity(guest: GuestIdentity, name: &str, script: &str) -> Option<String> {
    let line = match guest {
        GuestIdentity::InstanceName => {
            format!("hostnamectl set-hostname {name} || hostname {name}\n")
        }
        GuestIdentity::ContainerName => {
            format!("export {}={name}\n", crate::config::WORKER_NAME_ENV)
        }
        GuestIdentity::ContainerEnv(variable) => {
            format!(
                "export {}=\"${variable}\"\n",
                crate::config::WORKER_NAME_ENV
            )
        }
        GuestIdentity::NoStartupScript => return None,
    };
    Some(match script.split_once('\n') {
        Some((first, rest)) if first.starts_with("#!") => format!("{first}\n{line}{rest}"),
        _ => format!("#!/bin/bash\n{line}{script}"),
    })
}

/// An instance name the guest and every vendor accept as a hostname.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[async_trait]
impl Provider for GpuCloudProvider {
    #[allow(clippy::too_many_arguments)]
    async fn create_instance(
        &self,
        name: &str,
        machine_type: &str,
        accel_type: &str,
        boot_disk_gb: i64,
        _image: &str,
        _image_project: &str,
        startup_script: &str,
        preemptible: bool,
    ) -> Result<Option<String>, ProviderError> {
        let vendor = self.vendor.display_name();
        if preemptible {
            return Err(ProviderError::Value(format!(
                "{vendor}: the adapter rents on-demand machines only and was asked for a \
                 preemptible one"
            )));
        }
        if machine_type.trim().is_empty() {
            return Err(ProviderError::Value(format!(
                "{vendor}: no instance type was given for accelerator {accel_type:?}; the \
                 vendor's offers are {:?}",
                profile(self.vendor).offers
            )));
        }
        if !valid_name(name) {
            return Err(ProviderError::Value(format!(
                "{vendor}: instance name {name:?} must be lowercase letters, digits and dashes"
            )));
        }
        let Some(script) = with_identity(profile(self.vendor).guest, name, startup_script) else {
            return Err(ProviderError::Value(format!(
                "{vendor}: the vendor's deploy API takes no startup script, so Stado cannot boot \
                 an agent on a machine it rents there; {} machines are listed, read and released \
                 but never dispatched",
                self.vendor.provider()
            )));
        };
        let request = LaunchRequest {
            name,
            instance_type: machine_type,
            accel_type,
            boot_disk_gb,
            startup_script: &script,
        };
        match self.api.launch(&request).await {
            Ok(machine) => {
                let reference = self.reference(&machine);
                self.log(&format!("launched {reference} type={machine_type}"));
                Ok(Some(reference))
            }
            Err(error @ GpuCloudError::Capacity { .. }) => {
                self.log(&format!("{error}"));
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn delete_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        let native = self.native_id(instance_ref)?;
        match self.api.terminate(native).await {
            Ok(()) => {
                self.log(&format!("released {instance_ref}"));
                Ok(())
            }
            Err(GpuCloudError::NotFound { .. }) => {
                self.log(&format!("{instance_ref} was already gone"));
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn instance_exists(&self, instance_ref: &str) -> Result<bool, ProviderError> {
        let native = self.native_id(instance_ref)?;
        Ok(self
            .api
            .machine(native)
            .await?
            .is_some_and(|machine| machine.state.alive()))
    }

    async fn instance_lifecycle_state(
        &self,
        instance_ref: &str,
    ) -> Result<Option<String>, ProviderError> {
        let native = self.native_id(instance_ref)?;
        Ok(self
            .api
            .machine(native)
            .await?
            .map(|machine| machine.state.lifecycle().to_string()))
    }

    async fn list_running_instances(&self) -> Result<BTreeMap<String, i64>, ProviderError> {
        let mut by_accel: BTreeMap<String, Vec<Machine>> = BTreeMap::new();
        for machine in self.live_agents().await? {
            by_accel
                .entry(self.accel_of(&machine.instance_type))
                .or_default()
                .push(machine);
        }
        by_accel
            .into_iter()
            .map(|(accel, machines)| {
                i64::try_from(machines.len())
                    .map(|count| (accel, count))
                    .map_err(|error| ProviderError::Value(error.to_string()))
            })
            .collect()
    }

    async fn list_running_instance_refs_with_age(
        &self,
    ) -> Result<Vec<(String, f64)>, ProviderError> {
        let now = Utc::now();
        let mut refs = Vec::new();
        for machine in self.live_agents().await? {
            let Some(created) = machine.created_at else {
                return Err(ProviderError::Value(format!(
                    "{}: machine {} reports no creation time, so its age is unknown and the \
                     reaper cannot judge it",
                    self.vendor.display_name(),
                    machine.native_id
                )));
            };
            // A vendor clock slightly ahead of ours reports a machine created
            // "after now"; that machine has existed for no time at all.
            let age = (now - created)
                .max(chrono::TimeDelta::zero())
                .to_std()
                .map_err(|error| ProviderError::Value(error.to_string()))?
                .as_secs_f64();
            refs.push((self.reference(&machine), age));
        }
        Ok(refs)
    }
}
