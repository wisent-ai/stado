use super::*;

impl Fixture {
    pub(crate) fn bootstrap(&self) {
        let output = Command::new("/bin/launchctl")
            .args(["bootstrap", &self.domain])
            .arg(&self.plist)
            .output()
            .expect("launchctl bootstrap runs");
        assert!(
            output.status.success(),
            "launchctl bootstrap failed: {}",
            said(&output)
        );
    }

    pub(crate) fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", PATH)
            .env("TMPDIR", self.home.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_STADO_STORAGE_NAMESPACE", "probierz-native-readers")
            .env("STADO_CONFIG", &self.config);
        command
    }

    pub(crate) fn converge(&self) -> Output {
        self.command(&[
            "release",
            "converge-local-readers",
            "--name",
            "stado",
            "--archive",
            self.archive.to_str().expect("fixture archive path"),
            "--sha256",
            &self.archive_sha256,
        ])
        .output()
        .expect("built Stado convergence command runs")
    }

    pub(crate) fn update_private_reader(&self) -> Output {
        self.command(&[
            "service",
            "update",
            &self.label,
            "--host",
            HOST,
            "--from-archive",
            self.archive.to_str().expect("fixture archive path"),
            "--refresh-image",
            "--json",
        ])
        .output()
        .expect("built Stado service update command runs")
    }
    pub(crate) fn label_print(&self) -> Output {
        self.command(&[
            "service",
            "label-print",
            &self.label,
            "--host",
            HOST,
            "--domain",
            "user",
            "--json",
        ])
        .output()
        .expect("public label-print command runs")
    }

    pub(crate) fn observed_image(&self, expected_pid: u32) -> serde_json::Value {
        let output = self.label_print();
        assert!(
            output.status.success(),
            "label-print failed for pid {expected_pid}: {}",
            said(&output)
        );
        let report: serde_json::Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                panic!("label-print returned non-JSON ({error}): {}", said(&output))
            });
        assert_eq!(
            report["loaded"], true,
            "label-print did not find the loaded fixture"
        );
        assert_eq!(
            report["domain"], self.domain,
            "label-print did not report the observed launchd owner"
        );
        assert_eq!(
            report["pid"],
            expected_pid.to_string(),
            "label-print and launchd disagree about the live pid"
        );
        assert_eq!(
            report["process_identity_unavailable"],
            serde_json::Value::Null,
            "the public reader could not establish the live process identity: {report}"
        );
        report
    }

    pub(crate) fn pid(&self) -> Option<u32> {
        self.launchd_pid(&self.label)
    }

    /// The pid launchd itself holds for `label` in the fixture domain, or
    /// `None` when the domain does not hold the label or holds it idle.
    pub(crate) fn launchd_pid(&self, label: &str) -> Option<u32> {
        let service = format!("{}/{label}", self.domain);
        let output = Command::new("/bin/launchctl")
            .args(["print", &service])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| line.trim().strip_prefix("pid = ")?.trim().parse().ok())
    }

    pub(crate) fn wait_for_pid(&self, different_from: Option<u32>, budget: Duration) -> u32 {
        let deadline = Instant::now() + budget;
        loop {
            if let Some(pid) = self.pid() {
                if different_from != Some(pid) {
                    return pid;
                }
            }
            assert!(
                Instant::now() < deadline,
                "{} never acquired a replacement pid different from {different_from:?}",
                self.label
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub(crate) fn wait_until_listening(&self, budget: Duration) {
        let address = format!("127.0.0.1:{}", self.port).parse().unwrap();
        let deadline = Instant::now() + budget;
        loop {
            if TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the fixture dashboard never listened on {address}: {}\nlaunchd:\n{}",
                fs::read_to_string(self.home.join("native-readers.stderr.log")).unwrap_or_default(),
                said(
                    &Command::new("/bin/launchctl")
                        .args(["print", &format!("{}/{}", self.domain, self.label)])
                        .output()
                        .expect("read failed fixture ownership")
                )
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub(crate) fn assert_dashboard_serves_product_route(&self) {
        let mut stream =
            TcpStream::connect(("127.0.0.1", self.port)).expect("connect to fixture dashboard");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("dashboard read timeout");
        stream
            .write_all(b"GET /join.sh HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .expect("write dashboard request");
        let mut answer = String::new();
        stream
            .read_to_string(&mut answer)
            .expect("read dashboard answer");
        assert!(
            answer.starts_with("HTTP/1.1 200 OK"),
            "the real dashboard did not serve /join.sh: {answer}"
        );
    }

    pub(crate) fn declared_program(&self) -> String {
        let output = Command::new("/usr/bin/plutil")
            .args(["-extract", "ProgramArguments.0", "raw", "-o", "-"])
            .arg(&self.plist)
            .output()
            .expect("plutil reads the fixture declaration");
        assert!(output.status.success(), "plutil failed: {}", said(&output));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    pub(crate) fn cleanup(&mut self) -> Result<(), String> {
        let mut failures = Vec::new();
        match self
            .command(&[
                "service",
                "bootout",
                &self.label,
                "--host",
                HOST,
                "--domain",
                "user",
                "--json",
            ])
            .output()
        {
            Ok(bootout) => {
                let bootout_report: Option<serde_json::Value> =
                    serde_json::from_slice(&bootout.stdout).ok();
                let bootout_state = bootout_report
                    .as_ref()
                    .and_then(|report| report["state"].as_str());
                if !bootout.status.success()
                    || !matches!(bootout_state, Some("booted_out" | "absent"))
                {
                    failures.push(format!(
                        "Stado service bootout did not prove cleanup: {}",
                        said(&bootout)
                    ));
                }
            }
            Err(error) => failures.push(format!("Stado cleanup bootout did not run: {error}")),
        }

        // A lifecycle refusal is retained as a failure even if the exact-owner
        // launchctl fallback protects the host from a leaked KeepAlive job.
        if !failures.is_empty() || self.pid().is_some() {
            let service = format!("{}/{}", self.domain, self.label);
            match Command::new("/bin/launchctl")
                .args(["bootout", &service])
                .output()
            {
                Ok(fallback) if fallback.status.success() && self.pid().is_none() => {}
                Ok(fallback) => failures.push(format!(
                    "exact-owner fallback did not remove {service}: {}",
                    said(&fallback)
                )),
                Err(error) => failures.push(format!(
                    "fallback launchctl bootout did not run for {service}: {error}"
                )),
            }
        }

        let plist = self.plist.to_string_lossy().into_owned();
        match self
            .command(&["space", "file", "remove", HOST, &plist, "--json"])
            .output()
        {
            Ok(remove) => {
                let remove_report: Option<serde_json::Value> =
                    serde_json::from_slice(&remove.stdout).ok();
                let remove_status = remove_report
                    .as_ref()
                    .and_then(|report| report["status"].as_str());
                if !remove.status.success()
                    || !matches!(remove_status, Some("removed" | "absent"))
                    || self.plist.exists()
                {
                    failures.push(format!(
                        "Stado guarded space file remove did not prove cleanup: {}",
                        said(&remove)
                    ));
                }
            }
            Err(error) => failures.push(format!(
                "Stado guarded space file remove did not run: {error}"
            )),
        }

        if let Err(error) = self.cleanup_idle() {
            failures.push(error);
        }

        if failures.is_empty() {
            self.cleanup_finished = true;
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }
}
