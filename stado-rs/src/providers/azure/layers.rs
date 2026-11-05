//! The security groups that stand between the Internet and one interface,
//! read the way Azure applies them, and the allow that opens one port to
//! that interface alone.
//!
//! Inbound traffic passes the subnet's security group first and then the
//! interface's, and each must allow it
//! (<https://learn.microsoft.com/en-us/azure/virtual-network/network-security-group-how-it-works>).
//! A group without a rule that matches ends in its default `DenyAllInBound`,
//! so an interface whose own group allows 80 and 443 still gets nothing when
//! the subnet carries a group with no allow for them. Every layer is read and
//! judged here, so a command names the group that drops a port before it
//! connects to anything.

use std::net::Ipv4Addr;

use serde_json::{json, Value};

use super::public::{free_priority, inbound_rule_properties, Inbound};
use super::{ArmClient, NETWORK_API_VERSION};

/// Azure's last inbound rule in every group, which drops what no rule above
/// it allowed (same page).
const DEFAULT_DENY: &str = "DenyAllInBound (Azure's default)";

/// The source prefixes that match a connection from anywhere on the Internet.
const INTERNET_PREFIXES: [&str; 3] = ["*", "Internet", "0.0.0.0/0"];

/// What one group decides for one port.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub port: u16,
    pub allowed: bool,
    /// The rule that decided, by name, or [`DEFAULT_DENY`].
    pub rule: String,
}

/// One security group in the path, and what it decides for each port.
#[derive(Debug, Clone)]
pub struct Layer {
    /// `subnet` or `interface`: the order Azure applies them in.
    pub level: &'static str,
    pub group_id: String,
    pub decisions: Vec<Decision>,
}

impl Layer {
    pub fn group_name(&self) -> &str {
        last_segment(&self.group_id)
    }

    /// The ports this group drops.
    pub fn drops(&self) -> Vec<u16> {
        self.decisions
            .iter()
            .filter(|decision| !decision.allowed)
            .map(|decision| decision.port)
            .collect()
    }

    pub fn report(&self) -> Value {
        json!({
            "level": self.level,
            "group": self.group_name(),
            "group_id": self.group_id,
            "decisions": self.decisions.iter().map(|decision| json!({
                "port": decision.port,
                "allowed": decision.allowed,
                "rule": decision.rule,
            })).collect::<Vec<_>>(),
        })
    }
}

/// The path from the Internet to one interface.
#[derive(Debug, Clone)]
pub struct Path {
    pub interface: String,
    pub private_address: Ipv4Addr,
    /// Subnet first, then the interface: Azure's order.
    pub layers: Vec<Layer>,
}

impl Path {
    pub fn report(&self) -> Value {
        json!({
            "interface": self.interface,
            "private_address": self.private_address.to_string(),
            "layers": self.layers.iter().map(Layer::report).collect::<Vec<_>>(),
        })
    }
}

fn last_segment(id: &str) -> &str {
    id.rsplit('/').next().unwrap_or(id)
}

/// The resource group an ARM id names.
fn resource_group_of(id: &str) -> Option<&str> {
    let mut parts = id.split('/');
    while let Some(part) = parts.next() {
        if part.eq_ignore_ascii_case("resourceGroups") {
            return parts.next();
        }
    }
    None
}

fn arm_path(id: &str, child: &str) -> String {
    format!("{id}{child}?api-version={NETWORK_API_VERSION}")
}

/// Whether one port is inside a rule's `destinationPortRange(s)`.
fn port_matches(rule: &Value, port: u16) -> bool {
    let mut ranges: Vec<&str> = Vec::new();
    if let Some(range) = rule.get("destinationPortRange").and_then(Value::as_str) {
        ranges.push(range);
    }
    if let Some(many) = rule.get("destinationPortRanges").and_then(Value::as_array) {
        ranges.extend(many.iter().filter_map(Value::as_str));
    }
    ranges.iter().any(|range| match range.split_once('-') {
        _ if *range == "*" => true,
        Some((low, high)) => match (low.trim().parse::<u16>(), high.trim().parse::<u16>()) {
            (Ok(low), Ok(high)) => (low..=high).contains(&port),
            _ => false,
        },
        None => range.trim().parse::<u16>() == Ok(port),
    })
}

/// The prefixes of one side of a rule: `<side>AddressPrefix` and
/// `<side>AddressPrefixes`.
fn prefixes<'a>(rule: &'a Value, side: &str) -> Vec<&'a str> {
    let mut found: Vec<&str> = Vec::new();
    if let Some(single) = rule
        .get(format!("{side}AddressPrefix"))
        .and_then(Value::as_str)
    {
        found.push(single);
    }
    if let Some(many) = rule
        .get(format!("{side}AddressPrefixes"))
        .and_then(Value::as_array)
    {
        found.extend(many.iter().filter_map(Value::as_str));
    }
    found
}

/// Whether `address` is inside `prefix`: an address, a CIDR range, `*`, or
/// `VirtualNetwork`, which holds every address of the interface's network.
fn contains(prefix: &str, address: Ipv4Addr) -> bool {
    if prefix == "*" || prefix == "VirtualNetwork" {
        return true;
    }
    let (network, bits) = match prefix.split_once('/') {
        Some((network, bits)) => (network, bits.parse::<u32>().ok()),
        None => (prefix, Some(32)),
    };
    match (network.parse::<Ipv4Addr>(), bits) {
        (Ok(network), Some(bits)) if bits <= 32 => {
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            u32::from(network) & mask == u32::from(address) & mask
        }
        _ => false,
    }
}

/// Whether a rule matches a TCP connection from the Internet to `port` on
/// `address`, an interface in the application security groups `groups`.
fn matches(rule: &Value, port: u16, address: Ipv4Addr, groups: &[String]) -> bool {
    let inbound = rule.get("direction").and_then(Value::as_str) == Some("Inbound");
    let tcp = matches!(
        rule.get("protocol").and_then(Value::as_str),
        Some("Tcp") | Some("*")
    );
    // A rule whose source is an application security group admits members of
    // that group only, and an Internet visitor is in none.
    let from_internet = prefixes(rule, "source")
        .iter()
        .any(|prefix| INTERNET_PREFIXES.contains(prefix));
    let destination_groups: Vec<&str> = rule
        .get("destinationApplicationSecurityGroups")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .filter_map(|group| group.get("id").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    let to_address = if destination_groups.is_empty() {
        prefixes(rule, "destination")
            .iter()
            .any(|prefix| contains(prefix, address))
    } else {
        destination_groups.iter().any(|wanted| {
            groups
                .iter()
                .any(|member| member.eq_ignore_ascii_case(wanted))
        })
    };
    inbound && tcp && from_internet && to_address && port_matches(rule, port)
}

/// What a security group (its ARM document) decides for a TCP connection
/// from the Internet to `port` on `address`: the custom rules in priority
/// order, the first that matches deciding, else Azure's default deny.
pub fn decide(group: &Value, port: u16, address: Ipv4Addr, groups: &[String]) -> Decision {
    let mut rules: Vec<(i64, &Value, &str)> = group
        .pointer("/properties/securityRules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|rule| {
            let properties = rule.get("properties")?;
            let priority = properties.get("priority").and_then(Value::as_i64)?;
            let name = rule
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("(unnamed)");
            Some((priority, properties, name))
        })
        .collect();
    rules.sort_by_key(|(priority, _, _)| *priority);
    for (priority, properties, name) in rules {
        if matches(properties, port, address, groups) {
            return Decision {
                port,
                allowed: properties.get("access").and_then(Value::as_str) == Some("Allow"),
                rule: format!("{name} (priority {priority})"),
            };
        }
    }
    Decision {
        port,
        allowed: false,
        rule: DEFAULT_DENY.to_string(),
    }
}

async fn read_group(
    client: &ArmClient,
    level: &'static str,
    id: &str,
    ports: &[u16],
    address: Ipv4Addr,
    groups: &[String],
) -> Result<Layer, String> {
    let group = client
        .get(&arm_path(id, ""), &format!("read security group {id}"))
        .await
        .map_err(|error| format!("the {level}'s security group {id} could not be read: {error}"))?;
    Ok(Layer {
        level,
        group_id: id.to_string(),
        decisions: ports
            .iter()
            .map(|port| decide(&group, *port, address, groups))
            .collect(),
    })
}

/// Read the path from the Internet to interface `interface` of resource
/// group `rg`, judging `ports`. `None` when Azure holds no such interface.
pub async fn read(
    client: &ArmClient,
    rg: &str,
    interface: &str,
    ports: &[u16],
) -> Result<Option<Path>, String> {
    let interface_path =
        super::public::network_path(client.subscription(), rg, "networkInterfaces", interface);
    let Some(document) = client
        .get_allow_404(&interface_path, &format!("read NIC {interface}"))
        .await
        .map_err(|error| format!("the interface {interface} could not be read: {error}"))?
    else {
        return Ok(None);
    };
    let configuration = document
        .pointer("/properties/ipConfigurations/0/properties")
        .ok_or_else(|| {
            format!(
                "Azure answered the interface {interface} without an IP configuration: {document}"
            )
        })?;
    let address = configuration
        .get("privateIPAddress")
        .and_then(Value::as_str)
        .and_then(|address| address.parse::<Ipv4Addr>().ok())
        .ok_or_else(|| {
            format!("Azure answered the interface {interface} without a private IPv4 address: {document}")
        })?;
    let groups: Vec<String> = configuration
        .get("applicationSecurityGroups")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .filter_map(|group| group.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut layers = Vec::new();
    if let Some(subnet) = configuration.pointer("/subnet/id").and_then(Value::as_str) {
        let declared = client
            .get(&arm_path(subnet, ""), &format!("read subnet {subnet}"))
            .await
            .map_err(|error| {
                format!("the interface's subnet {subnet} could not be read: {error}")
            })?;
        if let Some(id) = declared
            .pointer("/properties/networkSecurityGroup/id")
            .and_then(Value::as_str)
        {
            layers.push(read_group(client, "subnet", id, ports, address, &groups).await?);
        }
    }
    if let Some(id) = document
        .pointer("/properties/networkSecurityGroup/id")
        .and_then(Value::as_str)
    {
        layers.push(read_group(client, "interface", id, ports, address, &groups).await?);
    }
    Ok(Some(Path {
        interface: interface.to_string(),
        private_address: address,
        layers,
    }))
}

/// The name of the allow written for one port of one host.
pub fn allow_rule_name(host: &str, port: u16) -> String {
    format!("stado-{host}-{port}")
}

/// Make every layer of `path` allow `ports` from the Internet to the
/// interface's own private address only, so no other interface of the
/// subnet is opened. A group outside `rg` is refused, naming it and the rule
/// it needs, since Stado writes only what its own resource group holds.
/// Returns what was written, as `<group>: <rule> for <port>`.
pub async fn open(
    client: &ArmClient,
    rg: &str,
    host: &str,
    path: &Path,
) -> Result<Vec<String>, String> {
    let destination = path.private_address.to_string();
    let internet = ["Internet".to_string()];
    let mut written = Vec::new();
    for layer in &path.layers {
        for port in layer.drops() {
            let name = allow_rule_name(host, port);
            let needed = format!(
                "an inbound allow named {name} for TCP {port} from Internet to {destination} only"
            );
            let owner = resource_group_of(&layer.group_id).unwrap_or("(unnamed)");
            if !owner.eq_ignore_ascii_case(rg) {
                return Err(format!(
                    "the {} security group {} is in resource group {owner}, not {rg} (AZURE_RESOURCE_GROUP), \
                     so Stado does not write it; whoever holds it adds {needed}",
                    layer.level,
                    layer.group_name()
                ));
            }
            let current = client
                .get(
                    &arm_path(&layer.group_id, ""),
                    &format!("read security group {}", layer.group_id),
                )
                .await
                .map_err(|error| {
                    format!(
                        "the security group {} could not be read: {error}",
                        layer.group_id
                    )
                })?;
            let priority = free_priority(&current, &name)?;
            let port_text = port.to_string();
            let mut properties = inbound_rule_properties(&Inbound {
                name: &name,
                port: &port_text,
                sources: &internet,
                destination_groups: &[],
                priority,
            });
            properties["destinationAddressPrefix"] = json!(destination);
            client
                .put_lro(
                    &arm_path(&layer.group_id, &format!("/securityRules/{name}")),
                    &json!({ "properties": properties }),
                    &format!("allow {port} to {destination} in {}", layer.group_id),
                )
                .await
                .map_err(|error| {
                    format!(
                        "{needed} was not written to {}: {error}",
                        layer.group_name()
                    )
                })?;
            written.push(format!("{}: {name} for {port}", layer.group_name()));
        }
    }
    Ok(written)
}
