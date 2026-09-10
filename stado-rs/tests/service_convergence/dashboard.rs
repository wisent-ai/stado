//! The authenticated dashboard this suite drives: its client grant, its
//! isolated vault and the answers it gives.
use super::*;

pub(crate) struct ClientGrant {
    pub(crate) name: String,
    pub(crate) actions: Vec<&'static str>,
    pub(crate) bearer: String,
}

impl ClientGrant {
    pub(crate) fn new(name: &str, actions: Vec<&'static str>, bearer: String) -> Self {
        Self {
            name: name.to_string(),
            actions,
            bearer,
        }
    }

    pub(crate) fn item(&self) -> String {
        format!("{}-registry-api", self.name)
    }
}

pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

pub(crate) struct DashboardFixture {
    pub(crate) _root: tempfile::TempDir,
    pub(crate) home: PathBuf,
    pub(crate) storage: PathBuf,
    pub(crate) config: PathBuf,
    pub(crate) protected: PathBuf,
    pub(crate) stado: PathBuf,
    pub(crate) skarbiec: PathBuf,
    pub(crate) skarbiec_current: String,
    pub(crate) skarbiec_declared: String,
    pub(crate) address: SocketAddr,
    pub(crate) vault: Option<SkarbiecFixture>,
    pub(crate) verifier_mint: Option<Value>,
    pub(crate) verifier_capabilities: Option<String>,
    pub(crate) dashboard: Child,
}

impl DashboardFixture {
    pub(crate) fn wait_until_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if TcpStream::connect_timeout(&self.address, Duration::from_millis(200)).is_ok()
                && self
                    .request("GET", "/api/service/converge?target=readiness", None, "")
                    .status
                    == 401
            {
                return;
            }
            if let Some(status) = self.dashboard.try_wait().expect("read dashboard status") {
                panic!(
                    "dashboard exited before readiness with {status}: {}",
                    fs::read_to_string(self._root.path().join("dashboard.stderr.log"))
                        .unwrap_or_default()
                );
            }
            assert!(
                Instant::now() < deadline,
                "dashboard never listened at {}: {}",
                self.address,
                fs::read_to_string(self._root.path().join("dashboard.stderr.log"))
                    .unwrap_or_default()
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    pub(crate) fn request(
        &self,
        method: &str,
        target: &str,
        bearer: Option<&str>,
        body: &str,
    ) -> Answer {
        let mut stream = TcpStream::connect_timeout(&self.address, Duration::from_secs(5))
            .expect("connect to real dashboard");
        stream
            .set_read_timeout(Some(Duration::from_secs(180)))
            .expect("dashboard read timeout");
        let mut request = format!(
            "{method} {target} HTTP/1.1\r\nHost: stado.wisent.com\r\nX-Forwarded-Proto: https\r\nX-Forwarded-For: 203.0.113.10\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        if let Some(bearer) = bearer {
            request.push_str(&format!("Authorization: Bearer {bearer}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(body);
        stream
            .write_all(request.as_bytes())
            .expect("write dashboard request");
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .expect("dashboard answers and closes");
        let raw = String::from_utf8(raw).expect("dashboard answer is UTF-8");
        let (head, body) = raw
            .split_once("\r\n\r\n")
            .expect("dashboard answer has a head and body");
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u16>().ok())
            .expect("dashboard answer has an HTTP status");
        Answer {
            status,
            body: serde_json::from_str(body)
                .unwrap_or_else(|error| panic!("dashboard body is not JSON ({error}): {body}")),
        }
    }

    pub(crate) fn mint_verifier(&self, token_file_name: &str) -> Output {
        mint_verifier(
            &self.home,
            &self.storage,
            &self.config,
            self.vault
                .as_ref()
                .expect("authenticated fixture has a real Skarbiec vault")
                .gnupg_home(),
            self.vault
                .as_ref()
                .expect("authenticated fixture has a real Skarbiec vault")
                .vault_file(),
            self.verifier_capabilities
                .as_deref()
                .expect("authenticated fixture has verifier capabilities"),
            token_file_name,
        )
    }

    pub(crate) fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }
}

impl Drop for DashboardFixture {
    fn drop(&mut self) {
        let _ = self.dashboard.kill();
        let _ = self.dashboard.wait();
        drop(self.vault.take());
    }
}
