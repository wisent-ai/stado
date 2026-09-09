//! Real mobile-egress data path on the machine running these cases.
//!
//! `stado egress mobile serve` is a loopback HTTP forward proxy whose only job
//! is a source address: every upstream socket it opens is bound to the IPv4
//! address of one named interface, so a tethered phone carries the traffic
//! instead of the host's default route. That binding is the contract, and it is
//! observable without any carrier: the cases below name this machine's own
//! egress interface, put a listener on that interface's address, drive a real
//! request through the real proxy, and read the source address the upstream
//! actually saw.
//!
//! Nothing here is simulated. The proxy is the built binary, the sockets are
//! real sockets on real addresses, and every refusal sentence was copied from a
//! live run. Which interface to use is discovered from the kernel — never from
//! the product under test and never from a literal in this file.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::process::{Child, Command, Output, Stdio};
use std::thread;

use nix::ifaddrs::getifaddrs;

/// A public address the kernel is asked to route to. No packet is sent: a
/// connected UDP socket only makes the kernel pick the source address it would
/// use for egress, which is how the interface under test is discovered.
const ROUTE_PROBE: &str = "1.1.1.1:53";

/// The path the upstream expects to see after the proxy has rewritten the
/// absolute-form request target into origin form.
const UPSTREAM_PATH: &str = "/upstream-source-address";

/// An interface name the kernel cannot possibly hold: `getifaddrs` answers with
/// device names, and this is not one.
const NO_SUCH_INTERFACE: &str = "stado-egress-no-such-interface";

/// This machine's own egress interface: the address the kernel would send from,
/// and the device name that address belongs to.
///
/// Fails loudly when the machine has no routable IPv4 interface. A case that
/// cannot find the interface it is about has not passed.
fn egress_interface() -> (String, Ipv4Addr) {
    let probe = UdpSocket::bind("0.0.0.0:0").expect("an ephemeral UDP socket binds");
    probe
        .connect(ROUTE_PROBE)
        .unwrap_or_else(|error| panic!("the kernel has no route to {ROUTE_PROBE}: {error}"));
    let IpAddr::V4(chosen) = probe
        .local_addr()
        .expect("the probe socket has a local address")
        .ip()
    else {
        panic!("the kernel chose an IPv6 source address; this proxy is IPv4-only");
    };
    let addresses = getifaddrs().expect("the kernel lists its interface addresses");
    for address in addresses {
        let Some(socket) = address
            .address
            .and_then(|value| value.as_sockaddr_in().copied())
        else {
            continue;
        };
        if socket.ip() == chosen {
            return (address.interface_name, chosen);
        }
    }
    panic!("no interface holds the kernel's own egress address {chosen}");
}

fn unused_loopback_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("an ephemeral loopback port is available")
        .local_addr()
        .expect("the listener has an address")
        .port()
}

/// The built binary, with an environment that reaches no canonical store: this
/// command owns only a socket.
fn serve_command(arguments: &[&str], home: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command
        .args(arguments)
        .env("HOME", home)
        .env("STADO_CONFIG", home.join("no-such-config.json"))
        .env("NO_COLOR", "1");
    command
}

/// Start the proxy and return it once it has said which address it bound its
/// upstream sockets to.
fn start_proxy(interface: &str, port: u16, home: &std::path::Path) -> (Child, String) {
    let mut child = serve_command(
        &[
            "egress",
            "mobile",
            "serve",
            "--interface",
            interface,
            "--port",
            &port.to_string(),
        ],
        home,
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .expect("the built Stado binary starts");
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().expect("proxy stdout is captured"))
        .read_line(&mut line)
        .expect("proxy emits its readiness line");
    (child, line)
}

/// One upstream on the interface's own address, and the source address plus
/// request line it saw.
struct Upstream {
    address: SocketAddr,
    handle: thread::JoinHandle<(IpAddr, String)>,
}

impl Upstream {
    fn bind(interface_address: Ipv4Addr, body: &'static str) -> Self {
        let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(interface_address), 0))
            .unwrap_or_else(|error| {
                panic!("an upstream listener binds on {interface_address}: {error}")
            });
        let address = listener.local_addr().expect("the upstream has an address");
        let handle = thread::spawn(move || {
            let (mut stream, peer) = listener.accept().expect("the upstream accepts the proxy");
            let mut request = String::new();
            BufReader::new(stream.try_clone().expect("clone the accepted stream"))
                .read_line(&mut request)
                .expect("the upstream reads the forwarded request line");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("the upstream answers the proxy");
            (peer.ip(), request.trim_end().to_string())
        });
        Self { address, handle }
    }
}

/// Drive one forward-proxy request and hand back everything the client read.
///
/// Every socket here is on this machine and the upstream closes the connection
/// after it answers, so the read ends on its own.
fn through_proxy(proxy: SocketAddr, upstream: SocketAddr) -> String {
    let mut stream =
        TcpStream::connect(proxy).expect("the Stado mobile proxy accepts a connection");
    write!(
        stream,
        "GET http://{upstream}{UPSTREAM_PATH} HTTP/1.1\r\nHost: {upstream}\r\nConnection: close\r\n\r\n"
    )
    .expect("the proxy request is sent");
    let mut answer = String::new();
    stream
        .read_to_string(&mut answer)
        .expect("the proxy returns the complete response");
    answer
}

/// The contract: the source address of the upstream connection is the address
/// of the interface the operator named, and the absolute-form target the client
/// sent arrived in origin form.
#[test]
fn the_proxy_binds_every_upstream_socket_to_the_named_interface() {
    let home = tempfile::tempdir().expect("an isolated home exists");
    let (interface, address) = egress_interface();
    let body = "carried by the named interface";
    let upstream = Upstream::bind(address, body);
    let port = unused_loopback_port();
    let (mut proxy, readiness) = start_proxy(&interface, port, home.path());

    assert!(
        readiness.contains("mobile egress ready:")
            && readiness.contains(&interface)
            && readiness.contains(&address.to_string()),
        "the proxy did not report the interface and the address the kernel holds for it: \
         {readiness:?}",
    );

    let answer = through_proxy(
        format!("127.0.0.1:{port}").parse().expect("a loopback socket"),
        upstream.address,
    );
    let (source, request_line) = upstream
        .handle
        .join()
        .expect("the upstream thread reports what it saw");
    let _ = proxy.kill();
    proxy.wait().expect("the proxy process is reaped");

    assert_eq!(
        source,
        IpAddr::V4(address),
        "the upstream connection did not come from the named interface's address",
    );
    assert_eq!(
        request_line,
        format!("GET {UPSTREAM_PATH} HTTP/1.1"),
        "the proxy did not rewrite the absolute-form target into origin form",
    );
    assert!(
        answer.starts_with("HTTP/1.1 200 OK") && answer.ends_with(body),
        "the client did not receive the upstream's own answer: {answer:?}",
    );
}

/// A name the kernel holds no address for is refused before any socket exists,
/// and the refusal says how to make it real.
#[test]
fn an_interface_with_no_usable_address_is_refused_and_nothing_listens() {
    let home = tempfile::tempdir().expect("an isolated home exists");
    let port = unused_loopback_port();
    let refused = run_serve(NO_SUCH_INTERFACE, None, port, home.path());

    assert_eq!(refused.status.code(), Some(1), "{}", said(&refused));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains(&format!(
            "interface {NO_SUCH_INTERFACE} has no usable IPv4 address; \
             connect and trust the phone tether first"
        )),
        "the refusal must name the interface and the repair: {}",
        said(&refused),
    );
    assert!(
        TcpListener::bind(("127.0.0.1", port)).is_ok(),
        "a refused proxy left something listening on 127.0.0.1:{port}",
    );
}

/// Listening off loopback would offer this machine's proxy to its network. The
/// address refused here is this machine's own egress address, which is exactly
/// the plausible mistake.
#[test]
fn a_non_loopback_listen_address_is_refused_and_nothing_listens() {
    let home = tempfile::tempdir().expect("an isolated home exists");
    let (interface, address) = egress_interface();
    let port = unused_loopback_port();
    let refused = run_serve(&interface, Some(&address.to_string()), port, home.path());

    assert_eq!(refused.status.code(), Some(2), "{}", said(&refused));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains(
            "mobile egress may listen only on loopback; run Weles on the same Stado host"
        ),
        "the refusal must name the boundary it keeps: {}",
        said(&refused),
    );
    assert!(
        TcpListener::bind((address, port)).is_ok(),
        "a refused proxy left something listening on {address}:{port}",
    );
}

fn run_serve(
    interface: &str,
    bind: Option<&str>,
    port: u16,
    home: &std::path::Path,
) -> Output {
    let text = port.to_string();
    let mut arguments = vec![
        "egress",
        "mobile",
        "serve",
        "--interface",
        interface,
        "--port",
        &text,
    ];
    if let Some(bind) = bind {
        arguments.extend_from_slice(&["--bind", bind]);
    }
    serve_command(&arguments, home)
        .output()
        .expect("the built Stado binary starts")
}

fn said(output: &Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}
