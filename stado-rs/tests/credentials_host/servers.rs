//! Real broker and Stado API processes owned by one isolated credential journey.

use std::fs;
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use super::host::IsolatedHost;

pub(super) struct Server {
    child: Child,
    pub url: String,
}

impl Server {
    pub fn broker(host: &IsolatedHost) -> Self {
        Self::start(host, "broker", host.broker_command().arg("serve"))
    }

    pub fn object_api(host: &IsolatedHost) -> Self {
        Self::start(
            host,
            "object-api",
            host.command()
                .env("STADO_CONFIG", host.home.join(".config/stado/config.json"))
                .args(["dashboard", "--bind", "127.0.0.1"]),
        )
    }

    fn start(host: &IsolatedHost, name: &str, command: &mut Command) -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let error_log = host.home.join(format!("{name}.err"));
        let child = command
            .args(["--port", &port.to_string()])
            .stdout(fs::File::create(host.home.join(format!("{name}.out"))).unwrap())
            .stderr(fs::File::create(&error_log).unwrap())
            .spawn()
            .expect("start the real product server");
        let mut server = Self {
            child,
            url: format!("http://127.0.0.1:{port}"),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if server.child.try_wait().unwrap().is_some() {
                break;
            }
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return server;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "real {name} did not start: {}",
            fs::read_to_string(error_log).unwrap()
        );
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
