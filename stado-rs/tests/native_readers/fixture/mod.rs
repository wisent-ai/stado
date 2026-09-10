use super::*;

mod operations;

impl Fixture {
    pub(crate) fn new() -> Self {
        // Launchd readers use the product's work area, not the Documents tree.
        let run_root = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"))
            .join(".stado/work/native-reader-runs");
        fs::create_dir_all(&run_root).expect("native-reader run root");
        let root = tempfile::Builder::new()
            .prefix("stado-native-readers-")
            .tempdir_in(run_root)
            .expect("managed native-reader fixture");
        let home = root.path().join("home");
        let storage = root.path().join("storage");
        let config = root.path().join("stado-config.json");
        let agents = home.join("Library/LaunchAgents");
        let delivered = home.join(".stado/bin");
        let private = home.join(".stado/native-readers/private");
        for directory in [
            &home,
            &storage,
            &agents,
            &delivered,
            &private,
            &home.join("tmp"),
        ] {
            fs::create_dir_all(directory).expect("isolated fixture directory");
        }
        fs::write(&config, b"{}\n").expect("isolated Stado configuration");

        let root_binary = delivered.join("stado");
        let private_binary = private.join("stado");
        fs::copy(env!("CARGO_BIN_EXE_stado"), &root_binary)
            .expect("copy built Stado into the delivered root");
        fs::copy(env!("CARGO_BIN_EXE_stado"), &private_binary)
            .expect("copy built Stado into the private root");

        let archive = root.path().join("stado-readers.tar.gz");
        write_stado_archive(&archive, &root_binary, "stado");
        let archive_sha256 = file_identity(&archive).sha256;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after Unix epoch")
            .as_nanos();
        let label = format!(
            "com.wisent.probierz.native-readers.{}.{}",
            std::process::id(),
            unique
        );
        let domain = available_login_domain();
        let plist = agents.join(format!("{label}.plist"));
        let port = unused_loopback_port();

        let fixture = Self {
            _root: root,
            home,
            storage,
            config,
            label,
            domain,
            plist,
            root_binary,
            private_binary,
            archive,
            archive_sha256,
            port,
            cleanup_finished: false,
        };
        fixture.write_registry();
        fixture.write_plist(&fixture.private_binary);
        fixture
    }

    pub(crate) fn write_registry(&self) {
        let hostname = hostname();
        let short_hostname = hostname.trim_end_matches(".local").to_string();
        let document = serde_json::json!({
            "schema_version": 2,
            "targets": [{
                "name": HOST,
                "kind": "local",
                "ssh": null,
                "release_platform": "darwin-arm64",
                "hostnames": [hostname, short_hostname],
                "role": "interactive",
                "managed_versions": {},
                "services": [{
                    "name": self.label,
                    "unit": "",
                    "label": self.label,
                    "path": self.plist,
                    "kind": "launchd",
                    "managed_since": "2026-09-05T00:00:00Z"
                }]
            }],
            "coordinators": []
        });
        fs::write(
            self.storage.join("registry.json"),
            serde_json::to_vec_pretty(&document).expect("fixture registry JSON"),
        )
        .expect("isolated fixture registry");
    }

    pub(crate) fn write_plist(&self, program: &Path) {
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0">
    <dict>
      <key>Label</key><string>{label}</string>
      <key>ProgramArguments</key>
      <array>
        <string>{program}</string>
        <string>dashboard</string>
        <string>--enrollment-only</string>
        <string>--bind</string>
        <string>127.0.0.1</string>
        <string>--port</string>
        <string>{port}</string>
      </array>
      <key>EnvironmentVariables</key>
      <dict>
        <key>HOME</key><string>{home}</string>
        <key>PATH</key><string>{path}</string>
        <key>TMPDIR</key><string>{tmp}</string>
        <key>WC_STORAGE_BACKEND</key><string>local</string>
        <key>WC_LOCAL_STORAGE_PATH</key><string>{storage}</string>
        <key>WC_STADO_STORAGE_NAMESPACE</key><string>probierz-native-readers</string>
        <key>STADO_CONFIG</key><string>{config}</string>
      </dict>
      <key>KeepAlive</key><true/>
      <key>RunAtLoad</key><true/>
      <key>ProcessType</key><string>Background</string>
      <key>StandardOutPath</key><string>{stdout}</string>
      <key>StandardErrorPath</key><string>{stderr}</string>
    </dict>
    </plist>
    "#,
            label = xml(&self.label),
            program = xml(&program.to_string_lossy()),
            port = self.port,
            home = xml(&self.home.to_string_lossy()),
            path = PATH,
            tmp = xml(&self.home.join("tmp").to_string_lossy()),
            storage = xml(&self.storage.to_string_lossy()),
            config = xml(&self.config.to_string_lossy()),
            stdout = xml(&self
                .home
                .join("native-readers.stdout.log")
                .to_string_lossy()),
            stderr = xml(&self
                .home
                .join("native-readers.stderr.log")
                .to_string_lossy()),
        );
        fs::write(&self.plist, body).expect("native-reader LaunchAgent plist");
    }
}
