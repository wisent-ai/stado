//! The installer renders the same host options that the CLI executes.

use clap::ValueEnum;
use super::ServeArgs;

impl ServeArgs {
    pub(crate) fn arguments(&self) -> Vec<String> {
        // Attached values retain empty strings and leading hyphens when the
        // emitted declaration is parsed again by clap.
        let mut args = vec![
            "serve".to_string(),
            format!("--gpu-type={}", self.worker.gpu_type),
            format!("--kind={}", self.worker.kind),
            format!("--vast-price-gpu={}", self.worker.vast_price_gpu),
            format!("--vast-max-duration-s={}", self.worker.vast_max_duration_s),
        ];
        if self.run_worker {
            args.push("--worker".to_string());
        }
        if self.disk_cleanup {
            args.push("--disk-cleanup".to_string());
        }
        if let Some(interval) = self.failure_fixer_interval_seconds {
            args.push(format!("--failure-fixer-interval-seconds={interval}"));
        }
        if let Some(pattern) = &self.failure_fixer_command_pattern {
            args.push(format!("--failure-fixer-command-pattern={pattern}"));
        }
        if let Some(interval) = self.release_interval_seconds {
            args.push(format!("--release-interval-seconds={interval}"));
        }
        if let Some(interval) = self.health_interval_seconds {
            args.push(format!("--health-interval-seconds={interval}"));
        }
        if let Some(target) = &self.worker.target {
            args.push(format!("--target={target}"));
        }
        if self.worker.auto {
            args.push("--auto".to_string());
        }
        if self.worker.idle_shutdown {
            args.push("--idle-shutdown".to_string());
        }
        if self.worker.vast_auto_list {
            args.push("--vast-auto-list".to_string());
        }
        if self.resolver {
            args.push("--resolver".to_string());
        }
        if let Some(coordinator) = &self.coordinator {
            args.push(format!("--coordinator={coordinator}"));
        }
        if let Some(mode) = self.control_plane {
            let value = mode.to_possible_value().expect("coordinator modes are CLI values");
            args.push(format!("--control-plane={}", value.get_name()));
        }
        if let Some(interval) = self.control_plane_interval_seconds {
            args.push(format!("--control-plane-interval-seconds={interval}"));
        }
        if let Some(destination) = &self.forward_destination {
            args.push(format!("--forward-destination={destination}"));
        }
        if let Some(port) = self.forward_remote_port {
            args.push(format!("--forward-remote-port={port}"));
        }
        if let Some(port) = self.forward_local_port {
            args.push(format!("--forward-local-port={port}"));
        }
        if let Some(interval) = self.forward_interval_seconds {
            args.push(format!("--forward-interval-seconds={interval}"));
        }
        if self.api {
            args.push("--api".to_string());
        }
        if let Some(bind) = &self.bind {
            args.push(format!("--bind={bind}"));
        }
        if let Some(port) = self.port {
            args.push(format!("--port={port}"));
        }
        if let Some(storage) = &self.api_storage {
            args.push(format!("--api-storage={storage}"));
        }
        if self.watchdog {
            args.extend([
                "--watchdog".to_string(),
                format!("--watchdog-interval-seconds={}", self.watchdog_interval_seconds),
            ]);
            if let Some(bucket) = &self.watchdog_bucket {
                args.push(format!("--watchdog-bucket={bucket}"));
            }
        }
        args
    }
}
