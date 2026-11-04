//! What a renter's container could reach on this host, read before the host
//! is offered to renters.
//!
//! A marketplace runs a renter's image as an unprivileged container on
//! Docker's default bridge network (Vast's security page: separate
//! namespaces, network and file system isolation). Two things such a
//! container still reaches unless the host stops it: every TCP service of
//! the host that listens on all addresses or on the bridge's own address,
//! through the bridge gateway; and every address of the host's local
//! networks, through the bridge's NAT. A fleet host listens with its Stado
//! object API, its vault and its agent, and sits beside other machines, so
//! listing it unchecked hands a stranger's code a route to all of them.
//!
//! The check reads the host's listening sockets (`ss -ltnH`) and Docker's
//! user chain (`iptables -S DOCKER-USER`), where a host drops traffic from
//! containers before Docker's own rules accept it. A listener the bridge
//! reaches, or a host whose chain does not drop the private ranges from the
//! bridge, refuses listing; a chain that cannot be read refuses listing too,
//! naming the read, because an unread firewall is not a closed one.

use serde::Serialize;

/// Docker's default bridge interface, the one a marketplace container sits
/// on; its address is the gateway every such container reaches the host by.
const BRIDGE: &str = "docker0";

/// The private IPv4 ranges (RFC 1918) a container on the bridge reaches by
/// NAT: the host's local networks and every machine on them.
const PRIVATE_RANGES: [&str; 3] = ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"];

/// The iptables program by absolute path, so the sudoers line that grants
/// reading the chain names exactly the program run (Debian and Ubuntu
/// install it here).
const IPTABLES: &str = "/usr/sbin/iptables";

/// What a renter's container could reach on this host, and whether that
/// refuses listing.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Isolation {
    /// The bridge gateway address, when the host has the bridge.
    pub bridge_address: Option<String>,
    /// `address:port` of every host listener the bridge reaches.
    pub reachable_listeners: Vec<String>,
    /// The private ranges the user chain does not drop from the bridge.
    pub reachable_networks: Vec<String>,
    /// Each reading that could not be taken, with its cause.
    pub unread: Vec<String>,
}

impl Isolation {
    /// Whether a renter's container is kept from everything of the host's.
    pub fn isolated(&self) -> bool {
        self.reachable_listeners.is_empty()
            && self.reachable_networks.is_empty()
            && self.unread.is_empty()
    }

    /// The sentence a refusal carries: what the container reaches and why it
    /// could not be told.
    pub fn refusal(&self) -> Option<String> {
        if self.isolated() {
            return None;
        }
        let mut parts = Vec::new();
        if !self.reachable_listeners.is_empty() {
            parts.push(format!(
                "a renter's container reaches the host services {} through the {BRIDGE} gateway; \
                 bind them to 127.0.0.1 or drop the bridge's traffic to them in the DOCKER-USER chain",
                self.reachable_listeners.join(", ")
            ));
        }
        if !self.reachable_networks.is_empty() {
            parts.push(format!(
                "it reaches the private networks {} by NAT; add `iptables -I DOCKER-USER -i {BRIDGE} \
                 -d <range> -j DROP` for each",
                self.reachable_networks.join(", ")
            ));
        }
        for cause in &self.unread {
            parts.push(format!("not verified: {cause}"));
        }
        Some(parts.join("; "))
    }
}

/// The IPv4 address of `interface`, from `ip -4 -o addr show dev`.
fn interface_address(interface: &str) -> Result<Option<String>, String> {
    let output = std::process::Command::new("ip")
        .args(["-4", "-o", "addr", "show", "dev", interface])
        .output()
        .map_err(|error| format!("ip could not run: {error}"))?;
    if !output.status.success() {
        // `ip` refuses a device that does not exist: this host has no
        // bridge, so no marketplace container of Docker's default kind runs.
        return Ok(None);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .skip_while(|word| *word != "inet")
        .nth(1)
        .and_then(|cidr| cidr.split('/').next())
        .map(str::to_string))
}

/// Every TCP listener `ss -ltnH` prints whose local address the bridge
/// reaches: all addresses (`0.0.0.0`, `*`, `[::]`) or the bridge's own.
pub fn reachable_listeners(ss_output: &str, bridge_address: &str) -> Vec<String> {
    let mut reached = Vec::new();
    for line in ss_output.lines() {
        // State, Recv-Q, Send-Q, Local Address:Port, Peer Address:Port.
        let Some(local) = line.split_whitespace().nth(3) else {
            continue;
        };
        let Some((address, port)) = local.rsplit_once(':') else {
            continue;
        };
        let address = address.trim_matches(|c| c == '[' || c == ']');
        let address = address.split('%').next().unwrap_or(address);
        if matches!(address, "0.0.0.0" | "*" | "::") || address == bridge_address {
            let entry = format!("{address}:{port}");
            if !reached.contains(&entry) {
                reached.push(entry);
            }
        }
    }
    reached
}

/// The private ranges the `iptables -S DOCKER-USER` rules do not drop for
/// traffic entering from the bridge.
pub fn undropped_ranges(rules: &str) -> Vec<String> {
    PRIVATE_RANGES
        .iter()
        .filter(|range| {
            !rules.lines().any(|rule| {
                let words: Vec<&str> = rule.split_whitespace().collect();
                let has = |flag: &str, value: &str| {
                    words
                        .windows(2)
                        .any(|pair| pair[0] == flag && pair[1] == value)
                };
                has("-i", BRIDGE) && has("-d", range) && (has("-j", "DROP") || has("-j", "REJECT"))
            })
        })
        .map(|range| range.to_string())
        .collect()
}

/// Read what a renter's container could reach on this host.
pub fn measure() -> Isolation {
    let mut isolation = Isolation {
        bridge_address: None,
        reachable_listeners: Vec::new(),
        reachable_networks: Vec::new(),
        unread: Vec::new(),
    };
    let bridge = match interface_address(BRIDGE) {
        Ok(Some(address)) => address,
        Ok(None) => {
            isolation.unread.push(format!(
                "this host has no {BRIDGE} bridge, so where a marketplace container sits cannot \
                 be read; install the marketplace's daemon first"
            ));
            return isolation;
        }
        Err(cause) => {
            isolation
                .unread
                .push(format!("the {BRIDGE} address cannot be read: {cause}"));
            return isolation;
        }
    };
    isolation.bridge_address = Some(bridge.clone());
    match std::process::Command::new("ss").args(["-ltnH"]).output() {
        Ok(output) if output.status.success() => {
            isolation.reachable_listeners =
                reachable_listeners(&String::from_utf8_lossy(&output.stdout), &bridge);
        }
        Ok(output) => isolation.unread.push(format!(
            "ss -ltnH exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => isolation.unread.push(format!("ss could not run: {error}")),
    }
    // The chain is root's to read and the agent is not root. It is read
    // through passwordless sudo for this one fixed command, the way `stado
    // host reboot` runs its one command: a host grants exactly that line and
    // nothing wider. Without the grant the reading is unread, naming it.
    match std::process::Command::new("sudo")
        .args(["-n", IPTABLES, "-S", "DOCKER-USER"])
        .output()
    {
        Ok(output) if output.status.success() => {
            isolation.reachable_networks =
                undropped_ranges(&String::from_utf8_lossy(&output.stdout));
        }
        Ok(output) => isolation.unread.push(format!(
            "sudo -n {IPTABLES} -S DOCKER-USER exited {}: {}; grant the agent's user exactly this \
             command with the sudoers line `<user> ALL=(root) NOPASSWD: {IPTABLES} -S DOCKER-USER`",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => isolation
            .unread
            .push(format!("sudo could not run: {error}")),
    }
    isolation
}
