//! Host health beacon collection: the `link` block a host publishes about
//! itself.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>. STADO_CONFIG
//! points at a nonexistent path so the developer's real config can never leak
//! in, and `PATH` is the fixture's own bin directory ONLY — that is what makes
//! these tests say something. The collector resolves `pmset`, `log`,
//! `journalctl` and `tailscale` through `PATH`, so a `PATH` holding nothing
//! but the fixtures is a host where exactly the seeded tools exist. Nothing
//! here reads the real power log, the real tailnet, or the fleet's store.
//!
//! `host publish-beacon --print` is the collection under test: it merges the
//! block into the document and publishes nothing. The document it writes IS
//! the published state — the same bytes that would go on the wire — so these
//! tests assert on the parsed document plus the exit code, and on the exact
//! refusal sentences where a document cannot be read.
//!
//! Every fixture is captured output, not invention: the `pmset -g log` lines
//! and `log show --style ndjson` records come from this workspace's Mac, the
//! tailnet document from its `tailscale status --json` (peers trimmed to the
//! one that holds a direct path), and the journal lines from
//! `ubuntu-server-rtx-pro-6000`, including the real
//! `i40e ... eth0: NIC Link is Up` spelling of a link change there.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// What every fixture host calls itself. `system_hostname()` shells out to
/// `hostname`, so seeding it is what makes a beacon document "about this
/// host" on any machine the suite runs on.
const FIXTURE_HOSTNAME: &str = "beacon-probe-host.local";
/// The beacon slug of [`FIXTURE_HOSTNAME`]: leading label, lowercased.
const FIXTURE_HOST: &str = "beacon-probe-host";

/// Real `pmset -g log` transitions: a maintenance sleep at 09:24:26 -0700 and
/// the wake at 09:27:13 -0700, with an older sleep and DarkWake above them so
/// the newest-of-each-kind rule has something to be right about.
const PMSET_FIXTURE: &str = "#!/bin/sh\n/bin/cat <<'LOG'\n\
2026-08-17 09:23:10 -0700 Sleep\tEntering Sleep state due to 'Notification Wake Back to Sleep':TCPKeepAlive=active Using AC (Charge:32%) 31 secs\n\
2026-08-17 09:23:41 -0700 DarkWake\tDarkWake from Deep Idle [CDNP] : due to smc.sysState.Wake(0x70070000) wifibt Using AC (Charge:32%) 45 secs\n\
2026-08-17 09:24:26 -0700 Sleep\tEntering Sleep state due to 'Maintenance Sleep':TCPKeepAlive=active Using AC (Charge:34%) 167 secs\n\
2026-08-17 09:27:13 -0700 Wake\tWake from Deep Idle [CDNVA] : due to smc.sysState.Wake(0x70070000) lid Using AC (Charge:34%)\n\
LOG\n";

/// Real `tailscale status --json`, trimmed to the node itself and the one peer
/// holding a direct path — which in the live document is the very Mac mini
/// whose six minutes of silence this whole block exists to explain.
const TAILSCALE_DIRECT_FIXTURE: &str = r#"#!/bin/sh
/bin/cat <<'JSON'
{"BackendState":"Running",
 "Self":{"HostName":"Lukaszs-MacBook-Pro-5485","Relay":"sfo","CurAddr":"",
         "Addrs":["24.23.232.108:17612","[2601:646:300:1900:2831:2e25:692d:f973]:41641","10.0.0.234:41641"],
         "Online":true},
 "Peer":{"nodekey:6f0c":{"HostName":"Charles's Mac mini","Relay":"sfo","CurAddr":"10.0.0.253:41641","Online":true,"Active":true}}}
JSON
"#;

/// The same document with no peer holding a direct path: every path this host
/// has runs through its home DERP.
const TAILSCALE_RELAY_FIXTURE: &str = r#"#!/bin/sh
/bin/cat <<'JSON'
{"BackendState":"Running",
 "Self":{"HostName":"Lukaszs-MacBook-Pro-5485","Relay":"sfo","CurAddr":"",
         "Addrs":["24.23.232.108:17612"],"Online":true},
 "Peer":{"nodekey:6f0c":{"HostName":"Charles's Mac mini","Relay":"sfo","CurAddr":"","Online":true,"Active":false}}}
JSON
"#;

/// Real `log show --style ndjson` records from `configd`: the periodic wifi
/// association line, then a link transition.
const LOG_SHOW_FIXTURE: &str = r#"#!/bin/sh
/bin/cat <<'ND'
{"timestamp":"2026-08-19 11:59:32.869840-0700","processImagePath":"/usr/libexec/configd","subsystem":"com.apple.IPConfiguration","category":"Server","eventMessage":"en0: SSID <redacted> BSSID <redacted> NetworkID <redacted> Security WPA2_PSK ConnectionID 1"}
{"timestamp":"2026-08-19 12:04:11.204311-0700","processImagePath":"/usr/libexec/configd","subsystem":"com.apple.IPConfiguration","category":"Server","eventMessage":"en0: link inactive"}
ND
"#;

/// One `journalctl` standing in for both reads the collector makes, branching
/// on `-k` exactly as the real tool does: the kernel log for interface
/// changes, the suspend unit otherwise. The kernel line is verbatim from
/// `ubuntu-server-rtx-pro-6000`; the suspend lines carry systemd's own
/// `Starting`/`Finished` wording in that host's `short-iso` spelling.
const JOURNALCTL_FIXTURE: &str = r#"#!/bin/sh
for argument in "$@"; do
  if [ "$argument" = "-k" ]; then
    /bin/cat <<'KERNEL'
2026-08-17T19:46:46+00:00 ubuntu-server kernel: i40e 0000:01:00.0 eth0: NIC Link is Up, 1000 Mbps Full Duplex, Flow Control: None, EEE: Enabled
KERNEL
    exit 0
  fi
done
/bin/cat <<'SUSPEND'
2026-08-17T19:44:02+00:00 ubuntu-server systemd[1]: Starting systemd-suspend.service - System Suspend...
2026-08-17T19:46:41+00:00 ubuntu-server systemd[1]: Finished systemd-suspend.service - System Suspend.
SUSPEND
"#;

/// A host with a `PATH` of its own: storage, a beacon document, and exactly
/// the probe binaries a test seeds.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    /// A host that knows its own name and nothing else. Every probe is
    /// absent until a test seeds it.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        std::fs::create_dir_all(dir.path().join("storage")).unwrap();
        let fixture = Self { dir };
        fixture.seed(
            "hostname",
            &format!("#!/bin/sh\nprintf '{FIXTURE_HOSTNAME}\\n'\n"),
        );
        fixture
    }

    fn bin(&self) -> PathBuf {
        self.dir.path().join("bin")
    }

    fn storage(&self) -> PathBuf {
        self.dir.path().join("storage")
    }

    /// Put one executable probe on this host's `PATH`.
    fn seed(&self, name: &str, body: &str) {
        let path = self.bin().join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Every probe this platform's collector reads, so the block comes back
    /// complete on whichever host the suite runs on.
    fn seed_platform_probes(&self) {
        if cfg!(target_os = "macos") {
            self.seed("pmset", PMSET_FIXTURE);
            self.seed("log", LOG_SHOW_FIXTURE);
        } else {
            self.seed("journalctl", JOURNALCTL_FIXTURE);
        }
    }

    /// A beacon document naming `host`, in the shape the collector scripts
    /// publish: host, reported_at, disk, units.
    fn beacon(&self, host: &str) -> PathBuf {
        let path = self.dir.path().join(format!("{host}.json"));
        std::fs::write(
            &path,
            format!(
                r#"{{"host": "{host}",
                     "reported_at": "2026-08-19T19:00:00Z",
                     "disk": "/dev/disk3s1s1 1.8Ti 9.8Gi 200Gi 5% /",
                     "units": {{"com.wisent.host-health-beacon": {{"state": "loaded"}}}}}}"#
            ),
        )
        .unwrap();
        path
    }

    /// `stado host publish-beacon <document> --print` on this host.
    fn collect(&self, document: &Path) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args([
                "host",
                "publish-beacon",
                document.to_str().unwrap(),
                "--print",
            ])
            // Only the seeded probes exist on this host.
            .env("PATH", self.bin())
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.storage())
            // A set-but-missing STADO_CONFIG disables config-file discovery.
            .env("STADO_CONFIG", self.dir.path().join("no-such-config.json"))
            // Pin the log window so the probe argv is the same on every run.
            .env("WC_HEALTH_INTERVAL_SECONDS", "300")
            .env_remove("STADO_HOST_HEALTH_API_URL")
            .env_remove("STADO_HOST_HEALTH_API_TOKEN_FILE")
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("WC_PROFILES_DIR");
        command.output().expect("stado binary runs")
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The document the host would publish, parsed.
fn document(out: &Output) -> Value {
    assert!(out.status.success(), "collection failed: {}", stderr(out));
    serde_json::from_str(&stdout(out)).expect("the published document stays JSON")
}

/// The platform's `source` token when its log tool answered.
fn platform_source() -> &'static str {
    if cfg!(target_os = "macos") {
        "pmset+tailscale"
    } else {
        "journalctl+tailscale"
    }
}

#[test]
fn publish_beacon_carries_this_hosts_link_block() {
    let fixture = Fixture::new();
    fixture.seed_platform_probes();
    fixture.seed("tailscale", TAILSCALE_DIRECT_FIXTURE);

    let published = document(&fixture.collect(&fixture.beacon(FIXTURE_HOST)));
    // The rest of the beacon survives the merge untouched.
    assert_eq!(published["host"], FIXTURE_HOST);
    assert_eq!(published["reported_at"], "2026-08-19T19:00:00Z");
    assert_eq!(
        published["units"]["com.wisent.host-health-beacon"]["state"],
        "loaded"
    );

    let link = &published["link"];
    assert_eq!(link["source"], platform_source());
    // A peer holds a direct path, and the endpoint is THIS host's own LAN
    // spelling — the address a peer on the same network dials.
    assert_eq!(link["path_kind"], "direct");
    // The tailnet read is the same on every platform, so is this answer.
    assert_eq!(link["endpoint"], "10.0.0.234:41641");
    // collected_at dates the reading itself, so it is present and UTC.
    let collected_at = link["collected_at"].as_str().expect("collected_at is set");
    assert!(
        collected_at.ends_with('Z') && collected_at.len() == 20,
        "collected_at is not a UTC second stamp: {collected_at}"
    );

    if cfg!(target_os = "macos") {
        // The newest transition of each kind, converted out of pmset's
        // local-offset spelling: 09:24:26 -0700 and 09:27:13 -0700.
        assert_eq!(link["last_sleep_at"], "2026-08-17T16:24:26Z");
        assert_eq!(link["last_wake_at"], "2026-08-17T16:27:13Z");
        assert_eq!(
            link["interface_changes"],
            serde_json::json!([
                {
                    "at": "2026-08-19T18:59:32Z",
                    "detail": "en0: SSID <redacted> BSSID <redacted> NetworkID <redacted> Security WPA2_PSK ConnectionID 1"
                },
                {"at": "2026-08-19T19:04:11Z", "detail": "en0: link inactive"}
            ])
        );
    } else {
        assert_eq!(link["last_sleep_at"], "2026-08-17T19:44:02Z");
        assert_eq!(link["last_wake_at"], "2026-08-17T19:46:41Z");
        assert_eq!(
            link["interface_changes"],
            serde_json::json!([{
                "at": "2026-08-17T19:46:46Z",
                "detail": "kernel: i40e 0000:01:00.0 eth0: NIC Link is Up, 1000 Mbps Full Duplex, Flow Control: None, EEE: Enabled"
            }])
        );
    }
}

#[test]
fn publish_beacon_names_a_relayed_path_by_its_derp_region() {
    let fixture = Fixture::new();
    fixture.seed_platform_probes();
    fixture.seed("tailscale", TAILSCALE_RELAY_FIXTURE);

    let published = document(&fixture.collect(&fixture.beacon(FIXTURE_HOST)));
    let link = &published["link"];
    assert_eq!(link["path_kind"], "relay");
    assert_eq!(link["endpoint"], "derp:sfo");
    assert_eq!(link["source"], platform_source());
}

#[test]
fn publish_beacon_reports_unsupported_when_no_probe_answers() {
    // A host with no pmset, no log, no journalctl and no tailscale: every
    // datum is null and the block says so by name rather than inventing a
    // path or a sleep that nobody read.
    let fixture = Fixture::new();

    let published = document(&fixture.collect(&fixture.beacon(FIXTURE_HOST)));
    let link = &published["link"];
    assert_eq!(link["source"], "unsupported");
    assert_eq!(link["path_kind"], "unknown");
    assert_eq!(link["endpoint"], Value::Null);
    assert_eq!(link["last_sleep_at"], Value::Null);
    assert_eq!(link["last_wake_at"], Value::Null);
    assert_eq!(link["interface_changes"], serde_json::json!([]));
    assert!(
        link["collected_at"].is_string(),
        "an unsupported block still dates itself"
    );
}

#[test]
fn publish_beacon_leaves_a_relayed_document_without_this_hosts_link() {
    // The macOS collector relays beacons for hosts that cannot publish for
    // themselves. Stamping this machine's connectivity onto another
    // machine's document would fabricate exactly the evidence the block
    // exists to provide, so a document about somebody else stays as it came.
    let fixture = Fixture::new();
    fixture.seed_platform_probes();
    fixture.seed("tailscale", TAILSCALE_DIRECT_FIXTURE);

    let published = document(&fixture.collect(&fixture.beacon("charless-mac-mini")));
    assert_eq!(published["host"], "charless-mac-mini");
    assert_eq!(
        published.get("link"),
        None,
        "a relayed document must carry no link block: {published}"
    );
}

#[test]
fn publish_beacon_refuses_a_document_it_cannot_read() {
    let fixture = Fixture::new();
    fixture.seed_platform_probes();
    fixture.seed("tailscale", TAILSCALE_DIRECT_FIXTURE);

    let malformed = fixture.dir.path().join("malformed.json");
    std::fs::write(&malformed, "not json").unwrap();
    let out = fixture.collect(&malformed);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("Error: host beacon is not valid JSON: expected ident at line 1 column 2"),
        "got: {}",
        stderr(&out)
    );

    // A document with no units is not a beacon, and no amount of link
    // collection makes it one.
    let unitless = fixture.dir.path().join("unitless.json");
    std::fs::write(
        &unitless,
        r#"{"host": "beacon-probe-host", "reported_at": "2026-08-19T19:00:00Z"}"#,
    )
    .unwrap();
    let out = fixture.collect(&unitless);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out)
            .contains("Error: host beacon requires string reported_at and object units fields"),
        "got: {}",
        stderr(&out)
    );

    let misnamed = fixture.dir.path().join("misnamed.json");
    std::fs::write(
        &misnamed,
        r#"{"host": "Beacon_Probe_Host", "reported_at": "2026-08-19T19:00:00Z", "units": {}}"#,
    )
    .unwrap();
    let out = fixture.collect(&misnamed);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("Error: host beacon host must be a lowercase DNS label"),
        "got: {}",
        stderr(&out)
    );
}
