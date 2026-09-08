//! The last stretch of `config show`: where machines are created, and where
//! the dashboard listens once they are.

use serde_json::{Map, Value};

use crate::config;

pub(super) fn insert(resolved: &mut Map<String, Value>) {
    resolved.insert(
        "azure_resource_group".into(),
        Value::from(config::azure_resource_group()),
    );
    resolved.insert(
        "azure_locations".into(),
        Value::Array(
            config::azure_locations()
                .iter()
                .map(|l| Value::from(l.as_str()))
                .collect(),
        ),
    );
    resolved.insert(
        "dashboard_bind".into(),
        Value::from(config::dashboard_bind()),
    );
    resolved.insert(
        "dashboard_port".into(),
        Value::from(config::dashboard_port()),
    );
}
