//! Host-local egress processes managed by Stado services.
//!
//! Mobile egress is an HTTP CONNECT/forward proxy. It listens on loopback and
//! binds every upstream TCP connection to the IPv4 address of one named
//! interface, so Weles can use a tethered phone without inheriting the host's
//! default route. The service manager supplies persistence and restart policy;
//! this module owns only the data path.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use clap::Subcommand;
use nix::ifaddrs::getifaddrs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{lookup_host, TcpListener, TcpSocket, TcpStream};

use super::CmdError;

const MAX_HEADER_BYTES: usize = 16 * 1024;

#[derive(Subcommand)]
pub enum EgressCommands {
    /// Route Weles through a tethered phone interface.
    #[command(subcommand)]
    Mobile(MobileCommands),
}

#[derive(Subcommand)]
pub enum MobileCommands {
    /// Run a loopback HTTP proxy whose upstream sockets use one interface.
    Serve {
        /// Operating-system interface carrying the phone tether, for example en7.
        #[arg(long)]
        interface: String,
        /// Loopback address to listen on. Non-loopback binds are refused.
        #[arg(long, default_value = "127.0.0.1")]
        bind: IpAddr,
        /// Local proxy port consumed by Weles.
        #[arg(long, default_value_t = 8781)]
        port: u16,
    },
}

pub async fn dispatch(command: EgressCommands) -> Result<(), CmdError> {
    match command {
        EgressCommands::Mobile(MobileCommands::Serve {
            interface,
            bind,
            port,
        }) => serve_mobile(&interface, bind, port).await,
    }
}

fn interface_ipv4(interface: &str) -> Result<Ipv4Addr, CmdError> {
    let addresses = getifaddrs()
        .map_err(|error| CmdError::click(format!("cannot inspect network interfaces: {error}")))?;
    for address in addresses {
        if address.interface_name != interface {
            continue;
        }
        let Some(socket) = address.address.and_then(|value| value.as_sockaddr_in().copied()) else {
            continue;
        };
        let ip = socket.ip();
        if !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() {
            return Ok(ip);
        }
    }
    Err(CmdError::click(format!(
        "interface {interface} has no usable IPv4 address; connect and trust the phone tether first"
    )))
}

async fn serve_mobile(interface: &str, bind: IpAddr, port: u16) -> Result<(), CmdError> {
    if !bind.is_loopback() {
        return Err(CmdError::usage(
            "mobile egress may listen only on loopback; run Weles on the same Stado host",
        ));
    }
    let source = interface_ipv4(interface)?;
    let listener = TcpListener::bind(SocketAddr::new(bind, port))
        .await
        .map_err(|error| CmdError::click(format!("cannot listen on {bind}:{port}: {error}")))?;
    println!("mobile egress ready: http://{bind}:{port} via {interface} ({source})");

    loop {
        let (client, _) = listener.accept().await?;
        tokio::spawn(async move {
            if let Err(error) = proxy_connection(client, source).await {
                tracing::warn!(%error, "mobile egress connection failed");
            }
        });
    }
}

async fn read_header(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(1024);
    loop {
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(bytes);
        }
        if bytes.len() >= MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "proxy request header exceeds 16 KiB",
            ));
        }
        let read = stream.read_buf(&mut bytes).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "client closed before sending a complete proxy request",
            ));
        }
    }
}

fn split_first_line(header: &[u8]) -> io::Result<(&str, &[u8])> {
    let end = header
        .windows(2)
        .position(|window| window == b"\r\n")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing HTTP request line"))?;
    let line = std::str::from_utf8(&header[..end])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "request line is not UTF-8"))?;
    Ok((line, &header[end + 2..]))
}

fn authority_host_port(authority: &str, default_port: u16) -> io::Result<(String, u16)> {
    let candidate = format!("http://{authority}");
    let parsed = url::Url::parse(&candidate)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid proxy authority"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "proxy authority has no host"))?;
    Ok((host.to_owned(), parsed.port().unwrap_or(default_port)))
}

async fn connect_from(source: Ipv4Addr, host: &str, port: u16) -> io::Result<TcpStream> {
    let mut last_error = None;
    for destination in lookup_host((host, port)).await? {
        let SocketAddr::V4(destination) = destination else {
            continue;
        };
        let socket = TcpSocket::new_v4()?;
        socket.bind(SocketAddr::new(IpAddr::V4(source), 0))?;
        match socket.connect(SocketAddr::V4(destination)).await {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("{host}:{port} has no reachable IPv4 address"),
        )
    }))
}

async fn proxy_connection(mut client: TcpStream, source: Ipv4Addr) -> io::Result<()> {
    let header = read_header(&mut client).await?;
    let (line, remainder) = split_first_line(&header)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or_default();
    if target.is_empty() || !version.starts_with("HTTP/") || parts.next().is_some() {
        client
            .write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n")
            .await?;
        return Ok(());
    }

    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = authority_host_port(target, 443)?;
        let mut upstream = connect_from(source, &host, port).await?;
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
        return Ok(());
    }

    let parsed = url::Url::parse(target).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "forward proxy requests must use an absolute http:// URL",
        )
    })?;
    if parsed.scheme() != "http" {
        client
            .write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n")
            .await?;
        return Ok(());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "request URL has no host"))?;
    let port = parsed.port().unwrap_or(80);
    let mut upstream = connect_from(source, host, port).await?;
    let path = match parsed.query() {
        Some(query) => format!("{}?{query}", parsed.path()),
        None => parsed.path().to_owned(),
    };
    upstream
        .write_all(format!("{method} {path} {version}\r\n").as_bytes())
        .await?;
    upstream.write_all(remainder).await?;
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}
