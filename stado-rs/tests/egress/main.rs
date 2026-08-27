//! Real mobile-egress journey.
//!
//! Probierz runs this only on a Stado host with a trusted phone tether. No
//! proxy fixture or simulated network can establish the contract: the built
//! Stado binary must bind its upstream socket to the named interface, and the
//! public IP intelligence response must identify the resulting exit as mobile.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::Value;

const INTELLIGENCE_HOST: &str = "ip-api.com";
const INTELLIGENCE_PATH: &str =
    "/json/?fields=status,message,query,mobile,proxy,hosting,isp,org,as,countryCode";

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must name the trusted phone tether interface"))
}

fn unused_loopback_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("an ephemeral loopback port is available")
        .local_addr()
        .expect("the listener has an address")
        .port()
}

fn start_proxy(interface: &str, port: u16) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args([
            "egress",
            "mobile",
            "serve",
            "--interface",
            interface,
            "--port",
            &port.to_string(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the built Stado binary starts");
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().expect("proxy stdout is captured"))
        .read_line(&mut line)
        .expect("proxy emits its readiness line");
    assert!(
        line.contains("mobile egress ready:") && line.contains(interface),
        "unexpected readiness line: {line:?}",
    );
    child
}

fn request_through_proxy(proxy: SocketAddr) -> Value {
    let mut stream = TcpStream::connect_timeout(&proxy, Duration::from_secs(10))
        .expect("the Stado mobile proxy accepts a connection");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("the proxy read timeout is set");
    write!(
        stream,
        "GET http://{INTELLIGENCE_HOST}{INTELLIGENCE_PATH} HTTP/1.1\r\nHost: {INTELLIGENCE_HOST}\r\nConnection: close\r\n\r\n"
    )
    .expect("the proxy request is sent");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("the proxy returns the complete response");
    let separator = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("the response contains HTTP headers");
    let status = String::from_utf8_lossy(&response[..separator]);
    assert!(
        status.starts_with("HTTP/1.1 200") || status.starts_with("HTTP/1.0 200"),
        "{status}"
    );
    serde_json::from_slice(&response[separator + 4..]).expect("IP intelligence body is JSON")
}

#[test]
#[ignore = "Probierz supplies a real trusted phone tether on the selected Stado host"]
fn mobile_egress_uses_the_phone_interface_and_public_ip_is_mobile() {
    let interface = required("STADO_MOBILE_EGRESS_INTERFACE");
    let port = unused_loopback_port();
    let mut proxy = start_proxy(&interface, port);
    let assessment = request_through_proxy(format!("127.0.0.1:{port}").parse().unwrap());
    let _ = proxy.kill();
    let status = proxy.wait().expect("the proxy process is reaped");

    assert_eq!(assessment["status"], "success", "{assessment}");
    assert_eq!(
        assessment["mobile"], true,
        "exit is not classified as mobile: {assessment}"
    );
    assert_eq!(
        assessment["hosting"], false,
        "exit is classified as hosting: {assessment}"
    );
    assert_eq!(
        assessment["proxy"], false,
        "carrier exit is listed as a public proxy: {assessment}"
    );
    let ip = assessment["query"]
        .as_str()
        .expect("assessment carries the public IP");
    assert!(!ip.is_empty());
    assert!(
        !status.success(),
        "the long-running proxy stopped before test cleanup"
    );
    println!(
        "mobile egress verified: interface={interface}; public_ip={ip}; isp={}; country={}",
        assessment["isp"], assessment["countryCode"],
    );
}
