//! Putting the fleet into the state a story needs: a host with no core left
//! to give, and a reservation whose holder stopped answering long ago.

use std::fs::{self, File};
use std::path::PathBuf;
use std::process::Command;

use serde_json::json;

use super::{
    Journey, DEAD_RESERVATION_CPU_CORES, DEAD_RESERVATION_RAM_GB, DEAD_RESERVATION_SCHEMA_VERSION,
    DEAD_RESERVATION_TTL_SECONDS, FULL_HOST_AVAILABLE_CPU_CORES, FULL_HOST_FREE_RAM_GB,
    FULL_HOST_TOTAL_CPU_CORES, FULL_HOST_TOTAL_RAM_GB, LONG_EXPIRED, TARGET,
};

impl Journey {
    /// The consumer id this machine's agent publishes under.
    pub(crate) fn consumer_id(&self) -> String {
        let hostname =
            String::from_utf8(Command::new("hostname").arg("-f").output().unwrap().stdout)
                .unwrap()
                .trim()
                .to_ascii_lowercase();
        format!("local-{hostname}")
    }

    /// Publish this host as full: accepting, but with no core to give.
    pub(crate) fn publish_full_host(&self) {
        let consumer = self.consumer_id();
        let directory = self.storage.join("capacity");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(format!("{consumer}.json")),
            serde_json::to_string_pretty(&json!({
                "consumer_id": consumer,
                "kind": "local",
                "accepting_jobs": true,
                "running_jobs": 0,
                "running_workloads": 0,
                "total_cpu_cores": FULL_HOST_TOTAL_CPU_CORES,
                "available_cpu_cores": FULL_HOST_AVAILABLE_CPU_CORES,
                "available_accelerators": {},
                "free_ram_gb": FULL_HOST_FREE_RAM_GB,
                "total_ram_gb": FULL_HOST_TOTAL_RAM_GB,
                "free_vram_gb": 0,
                "total_vram_gb": 0,
                "reserved": {"cpu_cores": 0, "ram_gb": 0.0, "vram_gb": 0},
                "reservations": [],
                "published_at": chrono::Utc::now().to_rfc3339(),
                "diag": {},
                "stado_version": env!("CARGO_PKG_VERSION")
            }))
            .unwrap(),
        )
        .unwrap();
    }

    /// Seed one reservation in the product's own shape whose heartbeat is
    /// two hours old, and return its path.
    pub(crate) fn seed_dead_reservation(&self, kind: &str) -> PathBuf {
        let consumer = self.consumer_id();
        let directory = self.storage.join("state/reservations").join(&consumer);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("dead.json");
        let long_ago = chrono::Utc::now() - chrono::Duration::from_std(LONG_EXPIRED).unwrap();
        fs::write(
            &path,
            serde_json::to_string(&json!({
                "schema_version": DEAD_RESERVATION_SCHEMA_VERSION,
                "reservation_id": "dead",
                "consumer_id": consumer,
                "target": TARGET,
                "kind": kind,
                "product": "weles-worker",
                "holder": "a process that is gone",
                "cpu_cores": DEAD_RESERVATION_CPU_CORES,
                "ram_gb": DEAD_RESERVATION_RAM_GB,
                "vram_gb": 0,
                "acquired_at": long_ago.to_rfc3339(),
                "heartbeat_at": long_ago.to_rfc3339(),
                "ttl_seconds": DEAD_RESERVATION_TTL_SECONDS
            }))
            .unwrap(),
        )
        .unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - LONG_EXPIRED)
            .unwrap();
        path
    }
}
