//! A network's own node run on a fleet host as a declared Stado service:
//! the declaration, and the record of what the follow loop last did.
//!
//! Some sources do not rent a board to a stranger's container; the host runs
//! the network's own node (its release, its unit) and the network pays the
//! host for the work the node does. Stado does not install that node: the
//! operator declares it like any service, with `stado service declare`, from
//! the network's official release. A node declaration here names that
//! service, the host it is declared on, the source it earns from (an id of
//! `stado market sources`) and what the node holds on the host while it
//! runs, which the follow loop reserves through capacity like any workload.
//!
//! One JSON document per node under `state/market/nodes/<name>.json` in the
//! fleet's own store, the loop's last transition under
//! `state/market/node-runs/<name>.json`, and each finished run under
//! `state/market/node-hours/<name>/<started>.json`, so every host and Stado
//! Desktop read the same answer and a source's earnings can be divided by
//! the hours its node ran.

use serde::{Deserialize, Serialize};

use crate::queue::{JobStorage, Listing, StorageError};

/// Where the declarations live in the fleet's store.
pub const NODES_PREFIX: &str = "state/market/nodes/";
/// Where each node's last transition lives.
pub const RUNS_PREFIX: &str = "state/market/node-runs/";
/// Where each finished run of a node is kept.
pub const HOURS_PREFIX: &str = "state/market/node-hours/";

/// What a node operation can fail with, each with the sentence the operator
/// reads.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// The request itself is malformed.
    #[error("{0}")]
    Usage(String),
    /// The named node is not declared, or its declaration is unreadable.
    #[error("{0}")]
    NotFound(String),
    /// The request contradicts what is declared.
    #[error("{0}")]
    Refused(String),
    /// The fleet's store did not answer.
    #[error("{0}")]
    Storage(#[from] StorageError),
}

/// What the node holds on its host while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeReservation {
    pub cpu_cores: i64,
    pub ram_gb: f64,
    pub vram_gb: i64,
    /// The disk the node's data may take, in GiB: the follow loop starts the
    /// node only while this fits inside what the host may still write before
    /// the disk-full rule starts deleting, so a sold disk never eats it. A
    /// declaration from before disks were reserved reads as none.
    #[serde(default)]
    pub disk_gb: i64,
}

/// One network node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDeclaration {
    /// The node's name in this fleet.
    pub name: String,
    /// The source it earns from, by its id in `stado market sources`.
    pub source: String,
    /// The registry host the node's service is declared on.
    pub target: String,
    /// The service, as `stado service declare` named it on that host.
    pub service: String,
    pub reservation: NodeReservation,
    /// The fleet's own inference deployment this node serves to its
    /// network, when it is an inference network's node: the same engine,
    /// model and board the registry declares, offered only while the board
    /// carries no fleet work and the model's license chain is declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment: Option<String>,
    /// What the network requires of a host, as its own documentation states
    /// it: the follow loop starts the node only while the host measures up,
    /// naming the requirement it misses.
    #[serde(default, skip_serializing_if = "NodeRequirements::is_empty")]
    pub requirements: NodeRequirements,
    /// Whether the operator offers the node: the follow loop runs it only
    /// while this is true and the fleet's own queue is idle.
    pub listed: bool,
    pub declared_at: String,
    pub changed_at: String,
}

/// A network's published requirements of the host's connection, with the
/// page that states them. Each is optional: a network that states none is
/// held to none, and nothing here is a default Stado chose. Board memory is
/// not one: the node's own `vram_gb` reservation is admitted through
/// capacity, which refuses it where it does not fit.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeRequirements {
    /// Download and upload the host's measured connection must carry, in
    /// megabits per second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_download_mbps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_upload_mbps: Option<f64>,
    /// The page of the network's documentation the figures were read from;
    /// required with any of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl NodeRequirements {
    pub fn is_empty(&self) -> bool {
        self.min_download_mbps.is_none() && self.min_upload_mbps.is_none() && self.source.is_none()
    }

    fn validate(&self) -> Result<(), NodeError> {
        let stated = self.min_download_mbps.is_some() || self.min_upload_mbps.is_some();
        match (stated, self.source.as_deref().map(str::trim)) {
            (true, None | Some("")) => {
                return Err(NodeError::Usage(
                    "a network requirement needs --requirements-source, the page of the \
                     network's documentation that states it"
                        .to_string(),
                ))
            }
            (false, Some(_)) => {
                return Err(NodeError::Usage(
                    "--requirements-source names where requirements were read, and none was \
                     given"
                        .to_string(),
                ))
            }
            _ => {}
        }
        let positive = |mbps: Option<f64>| mbps.is_none_or(|mbps| mbps.is_finite() && mbps > 0.0);
        if !(positive(self.min_download_mbps) && positive(self.min_upload_mbps)) {
            return Err(NodeError::Usage(
                "--min-download-mbps and --min-upload-mbps must be above zero".to_string(),
            ));
        }
        Ok(())
    }

    /// Whether a measured connection meets the requirements, or which one
    /// it misses.
    pub fn met_by(&self, download_mbps: f64, upload_mbps: f64) -> Result<(), String> {
        let source = self.source.as_deref().unwrap_or("the network");
        if let Some(need) = self.min_download_mbps.filter(|need| download_mbps < *need) {
            return Err(format!(
                "the host's measured download is {download_mbps:.1} Mbit/s, below the \
                 {need} Mbit/s {source} requires"
            ));
        }
        if let Some(need) = self.min_upload_mbps.filter(|need| upload_mbps < *need) {
            return Err(format!(
                "the host's measured upload is {upload_mbps:.1} Mbit/s, below the {need} \
                 Mbit/s {source} requires"
            ));
        }
        Ok(())
    }
}

/// The follow loop's last transition for one node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeRun {
    pub node: String,
    /// Whether the loop left the node's service running.
    pub running: bool,
    /// The sentence the loop wrote with the transition.
    pub state: String,
    /// The capacity reservation held while it runs.
    pub reservation_id: Option<String>,
    /// When the current run began, while it runs.
    pub running_since: Option<String>,
    pub changed_at: String,
}

impl NodeDeclaration {
    /// Validate every field the operator supplied, before anything is
    /// written.
    pub fn validate(&self) -> Result<(), NodeError> {
        validate_name(&self.name)?;
        for (field, value) in [
            ("--source", &self.source),
            ("--target", &self.target),
            ("--service", &self.service),
        ] {
            if value.trim().is_empty() {
                return Err(NodeError::Usage(format!("{field} cannot be empty")));
            }
        }
        let held = self.reservation;
        if held.cpu_cores < 0
            || held.vram_gb < 0
            || held.disk_gb < 0
            || !(held.ram_gb.is_finite() && held.ram_gb >= 0.0)
        {
            return Err(NodeError::Usage(
                "--cpu-cores, --ram-gb, --vram-gb and --disk-gb are what the node holds and \
                 cannot be negative"
                    .to_string(),
            ));
        }
        if held.cpu_cores == 0 && held.vram_gb == 0 && held.ram_gb == 0.0 && held.disk_gb == 0 {
            return Err(NodeError::Usage(
                "a node that holds nothing cannot be kept off a busy board; declare its \
                 --cpu-cores, --ram-gb, --vram-gb or --disk-gb"
                    .to_string(),
            ));
        }
        self.requirements.validate()?;
        Ok(())
    }
}

/// A node name: lowercase letters, digits and inner hyphens, since it
/// becomes an object key and a role-lock name.
pub fn validate_name(name: &str) -> Result<(), NodeError> {
    crate::providers::peer::declaration::validate_name(name)
        .map_err(|error| NodeError::Usage(error.to_string().replace("peer name", "node name")))
}

fn path(name: &str) -> String {
    format!("{NODES_PREFIX}{name}.json")
}

fn run_path(name: &str) -> String {
    format!("{RUNS_PREFIX}{name}.json")
}

/// The declaration of `name`, or `None` when no node of that name exists.
pub async fn load(store: &JobStorage, name: &str) -> Result<Option<NodeDeclaration>, NodeError> {
    validate_name(name)?;
    let key = path(name);
    let Some(text) = store.download_text(&key).await? else {
        return Ok(None);
    };
    serde_json::from_str(&text).map(Some).map_err(|error| {
        NodeError::NotFound(format!(
            "{key} does not hold a node declaration: {error}; `stado market node remove {name}` \
             retires it and `stado market node add` declares it again"
        ))
    })
}

/// The declaration of `name`, refused with the declared names when absent.
pub async fn require(store: &JobStorage, name: &str) -> Result<NodeDeclaration, NodeError> {
    if let Some(declaration) = load(store, name).await? {
        return Ok(declaration);
    }
    let declared: Vec<String> = list(store)
        .await?
        .into_iter()
        .map(|node| node.name)
        .collect();
    let named = if declared.is_empty() {
        "no node is declared".to_string()
    } else {
        format!("declared: {}", declared.join(", "))
    };
    Err(NodeError::NotFound(format!(
        "no network node named {name} is declared ({named}); `stado market node add {name} \
         --source ID --target HOST --service NAME --cpu-cores N --ram-gb N --vram-gb N` declares \
         it"
    )))
}

/// Every declared node, by name.
pub async fn list(store: &JobStorage) -> Result<Vec<NodeDeclaration>, NodeError> {
    let mut nodes = Vec::new();
    for key in store.list_paths(NODES_PREFIX, Listing::Every).await? {
        let Some(name) = key
            .strip_prefix(NODES_PREFIX)
            .and_then(|rest| rest.strip_suffix(".json"))
        else {
            continue;
        };
        if let Some(node) = load(store, name).await? {
            nodes.push(node);
        }
    }
    nodes.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(nodes)
}

/// Write `declaration` after validating it, and read it back, so the answer
/// reports the document the store holds.
pub async fn save(
    store: &JobStorage,
    declaration: &NodeDeclaration,
) -> Result<NodeDeclaration, NodeError> {
    declaration.validate()?;
    let body = serde_json::to_string_pretty(declaration)
        .map_err(|error| NodeError::Usage(format!("the declaration cannot be encoded: {error}")))?;
    let key = path(&declaration.name);
    store.upload_text(&key, &body).await?;
    match load(store, &declaration.name).await? {
        Some(stored) if &stored == declaration => Ok(stored),
        Some(_) => Err(NodeError::Refused(format!(
            "{key} holds another declaration after the write: another writer changed it at the \
             same time; `stado market node show {}` reads what it holds now",
            declaration.name
        ))),
        None => Err(NodeError::Storage(StorageError::Other(format!(
            "{key} was written and is absent when read back"
        )))),
    }
}

/// Retire the declaration of `name` and its run record, and confirm both are
/// gone.
pub async fn remove(store: &JobStorage, name: &str) -> Result<(), NodeError> {
    for key in [path(name), run_path(name)] {
        store.delete_blob(&key).await?;
        if store.download_text(&key).await?.is_some() {
            return Err(NodeError::Storage(StorageError::Other(format!(
                "{key} was deleted and is still present when read back"
            ))));
        }
    }
    Ok(())
}

/// The loop's last transition for `name`, or `None` before its first.
pub async fn load_run(store: &JobStorage, name: &str) -> Result<Option<NodeRun>, NodeError> {
    let key = run_path(name);
    let Some(text) = store.download_text(&key).await? else {
        return Ok(None);
    };
    serde_json::from_str(&text).map(Some).map_err(|error| {
        NodeError::NotFound(format!("{key} does not hold a node run record: {error}"))
    })
}

/// Keep the loop's transition.
pub async fn save_run(store: &JobStorage, run: &NodeRun) -> Result<(), NodeError> {
    let body = serde_json::to_string_pretty(run)
        .map_err(|error| NodeError::Usage(format!("the run record cannot be encoded: {error}")))?;
    store.upload_text(&run_path(&run.node), &body).await?;
    Ok(())
}

/// One finished run of a node: the source it earned from, the host, and
/// when it ran.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeSegment {
    pub node: String,
    pub source: String,
    pub target: String,
    pub from: String,
    pub to: String,
}

impl NodeSegment {
    /// The hours of this run that fall at or after `since`.
    pub fn hours_since(&self, since: chrono::DateTime<chrono::Utc>) -> Result<f64, String> {
        let read = |at: &str| {
            chrono::DateTime::parse_from_rfc3339(at)
                .map(|at| at.with_timezone(&chrono::Utc))
                .map_err(|error| {
                    format!("node {} run time {at:?} is unreadable: {error}", self.node)
                })
        };
        let from = read(&self.from)?.max(since);
        let to = read(&self.to)?;
        Ok(((to - from).num_seconds().max(0)) as f64 / 3600.0)
    }
}

/// Keep one finished run.
pub async fn save_segment(store: &JobStorage, segment: &NodeSegment) -> Result<(), NodeError> {
    let body = serde_json::to_string(segment)
        .map_err(|error| NodeError::Usage(format!("the run segment cannot be encoded: {error}")))?;
    let key = format!(
        "{HOURS_PREFIX}{}/{}.json",
        segment.node,
        segment.from.replace(':', "")
    );
    store.upload_text(&key, &body).await?;
    Ok(())
}

/// Every kept run of every node.
pub async fn segments(store: &JobStorage) -> Result<Vec<NodeSegment>, NodeError> {
    let mut found = Vec::new();
    for key in store.list_paths(HOURS_PREFIX, Listing::Every).await? {
        let Some(text) = store.download_text(&key).await? else {
            continue;
        };
        found.push(serde_json::from_str(&text).map_err(|error| {
            NodeError::NotFound(format!("{key} does not hold a node run segment: {error}"))
        })?);
    }
    Ok(found)
}

/// The role lock one follow loop of `node` holds for its life.
pub fn loop_role(node: &str) -> String {
    format!("market-node--{node}")
}
