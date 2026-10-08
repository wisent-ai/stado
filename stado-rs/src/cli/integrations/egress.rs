//! Host-local egress processes managed by Stado services.
//!
//! Mobile egress is an HTTP CONNECT/forward proxy. It listens on loopback and
//! binds every upstream TCP connection to the IPv4 address of one named
//! interface, so Weles can use a tethered phone without inheriting the host's
//! default route. The service manager supplies persistence and restart policy;
//! this module owns only the data path.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;

use clap::Subcommand;
use nix::ifaddrs::getifaddrs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{lookup_host, TcpListener, TcpSocket, TcpStream};

use crate::cli::CmdError;

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
        /// Local proxy port the browser is pointed at; no port is assumed.
        #[arg(long)]
        port: u16,
        /// Maximum request header bytes, including the terminating blank line.
        /// The deployment must state its own bound; no byte budget is assumed.
        #[arg(long)]
        max_header_bytes: NonZeroUsize,
    },
}

pub async fn dispatch(command: EgressCommands) -> Result<(), CmdError> {
    match command {
        EgressCommands::Mobile(MobileCommands::Serve {
            interface,
            bind,
            port,
            max_header_bytes,
        }) => serve_mobile(&interface, bind, port, max_header_bytes.get()).await,
    }
}

fn interface_ipv4(interface: &str) -> Result<Ipv4Addr, CmdError> {
    let addresses = getifaddrs().map_err(|error| {
        CmdError::click(format!("cannot inspect network interfaces: {error}")).stating(
            crate::cli::entry::error::io_failure_code(std::io::Error::from(error).kind()),
        )
    })?;
    for address in addresses {
        if address.interface_name != interface {
            continue;
        }
        let Some(socket) = address
            .address
            .and_then(|value| value.as_sockaddr_in().copied())
        else {
            continue;
        };
        let ip = socket.ip();
        if !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() {
            return Ok(ip);
        }
    }
    Err(CmdError::refused(format!(
        "interface {interface} has no usable IPv4 address; connect and trust the phone tether first"
    )))
}

async fn serve_mobile(
    interface: &str,
    bind: IpAddr,
    port: u16,
    max_header_bytes: usize,
) -> Result<(), CmdError> {
    if !bind.is_loopback() {
        return Err(CmdError::usage(
            "mobile egress may listen only on loopback; run Weles on the same Stado host",
        ));
    }
    let source = interface_ipv4(interface)?;
    let listener = TcpListener::bind(SocketAddr::new(bind, port))
        .await
        .map_err(|error| {
            CmdError::click(format!("cannot listen on {bind}:{port}: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    let address = listener.local_addr()?;
    println!("mobile egress ready: http://{address} via {interface} ({source}); max_header_bytes={max_header_bytes}");

    loop {
        let (client, _) = listener.accept().await?;
        tokio::spawn(async move {
            if let Err(error) = proxy_connection(client, source, max_header_bytes).await {
                tracing::warn!(%error, "mobile egress connection failed");
            }
        });
    }
}

async fn read_header(stream: &mut TcpStream, limit: usize) -> io::Result<(Vec<u8>, usize)> {
    let terminator = b"\r\n\r\n";
    let mut bytes = Vec::new();
    let mut scan_from = bytes.len();
    loop {
        if let Some(position) = bytes[scan_from..]
            .windows(terminator.len())
            .position(|window| window == terminator)
        {
            let end = scan_from + position + terminator.len();
            return Ok((bytes, end));
        }
        if bytes.len() >= limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("proxy request header exceeds declared --max-header-bytes {limit}; received {} bytes without a complete header", bytes.len()),
            ));
        }
        // Revisit only the suffix where a split terminator could begin.
        let previous_len = bytes.len();
        scan_from = previous_len.saturating_sub(terminator.len());
        let remaining = limit - previous_len;
        (&mut *stream)
            .take(remaining as u64)
            .read_buf(&mut bytes)
            .await?;
        if bytes.len() == previous_len {
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

async fn proxy_connection(
    mut client: TcpStream,
    source: Ipv4Addr,
    max_header_bytes: usize,
) -> io::Result<()> {
    let (header, header_end) = read_header(&mut client, max_header_bytes).await?;
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
        upstream.write_all(&header[header_end..]).await?;
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
    // The http scheme declares its own default port; no number is written here.
    let port = parsed.port_or_known_default().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "request URL names no port and its scheme declares none")
    })?;
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
