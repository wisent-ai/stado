//! `--provider handtohuman`: a fleet host's capacity offered on the
//! HandToHuman marketplace, through HandToHuman's own CLI (`handtohuman
//! market …`, found on PATH), never its database.
//!
//! HandToHuman sells two kinds a Stado host supplies: `compute` (priced per
//! GPU-hour, dispatched back to this host through `stado submit
//! --pinned-host`) and `egress` (`residential_egress`, priced per gigabyte,
//! served by `handtohuman-egress` placed on this host). `--resource` names
//! which; a resource HandToHuman has no kind for (`cpu`, `disk`) is refused
//! naming the kinds it sells. The seller account is the one the host
//! declares (`market.handtohuman.seller`, `STADO_HANDTOHUMAN_SELLER`), and
//! the offer this host published is kept in the fleet store under
//! `market/handtohuman/<host>/<resource>.json`, so `unlist`, `status` and
//! `earnings` act on that offer and no other.

use serde_json::{json, Value};

use super::market::{Follow, ListOffer, MarketAdapter, ServeOrRent};
use crate::cli::CmdError;
use crate::queue::JobStorage;

/// What a host offers on HandToHuman, and the kind HandToHuman sells it as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Resource {
    Gpu,
    Egress,
    Cpu,
    Disk,
}

impl Resource {
    fn word(self) -> &'static str {
        match self {
            Resource::Gpu => "gpu",
            Resource::Egress => "egress",
            Resource::Cpu => "cpu",
            Resource::Disk => "disk",
        }
    }

    /// HandToHuman's offer kind and the unit its price is per, or why it
    /// sells no such resource.
    fn kind(self) -> Result<(&'static str, &'static str), CmdError> {
        match self {
            Resource::Gpu => Ok(("compute", "GPU-hour")),
            Resource::Egress => Ok(("residential-egress", "gigabyte")),
            Resource::Cpu | Resource::Disk => Err(CmdError::refused(format!(
                "HandToHuman sells no {} offer: its supplied kinds are compute (per GPU-hour) and \
                 residential egress (per gigabyte), so --resource takes gpu or egress there",
                self.word()
            ))),
        }
    }
}

/// The HandToHuman adapter, for the resource `--resource` names; readiness
/// needs none, every offer verb one.
pub(crate) struct HandToHumanAdapter {
    pub resource: Option<Resource>,
}

impl HandToHumanAdapter {
    /// The resource an offer verb acts on, or the refusal that names it.
    fn resource(&self, verb: &str) -> Result<Resource, CmdError> {
        let resource = self.resource.ok_or_else(|| {
            CmdError::usage(format!(
                "stado market {verb} --provider handtohuman needs --resource gpu or egress: what \
                 this host offers there"
            ))
        })?;
        resource.kind()?;
        Ok(resource)
    }
}

/// The seller account this host offers under.
fn seller() -> Result<String, CmdError> {
    let field = &crate::capabilities::HANDTOHUMAN_SELLER_CONFIG;
    crate::config_file::field_value(field)
        .and_then(|value| value.as_str().map(str::to_string))
        .filter(|seller| !seller.trim().is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} ({}) names no HandToHuman seller account, so this host offers nothing there: \
                 create one with `handtohuman market account create <id> --role worker --name …` \
                 and declare its id",
                field.path, field.env
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })
}

/// Run `handtohuman market ARGS` with `input` on its standard input, and
/// read the JSON line it prints.
async fn market_with(args: &[&str], input: Option<&str>) -> Result<Value, CmdError> {
    use tokio::io::AsyncWriteExt;
    let spoken = format!("handtohuman market {}", args.join(" "));
    let cannot_run = |error: std::io::Error| {
        CmdError::click(format!(
            "the handtohuman program could not run (`{spoken}`): {error}; install HandToHuman's \
             CLI on this host"
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound)
    };
    let mut command = tokio::process::Command::new("handtohuman");
    command
        .arg("market")
        .args(args)
        .stdin(if input.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(cannot_run)?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(cannot_run)?;
    }
    let output = crate::wait::child_output_async(child, &spoken)
        .await
        .map_err(cannot_run)?;
    if !output.status.success() {
        return Err(CmdError::refused(format!(
            "{spoken} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    serde_json::from_str(line).map_err(|error| {
        CmdError::declaration(format!("{spoken} printed {line:?}, not JSON: {error}"))
    })
}

async fn market(args: &[&str]) -> Result<Value, CmdError> {
    market_with(args, None).await
}

fn kept_key(host: &str, resource: Resource) -> String {
    format!("market/handtohuman/{host}/{}.json", resource.word())
}

/// The offer this host published for the resource, when it published one.
async fn kept_offer(store: &JobStorage, resource: Resource) -> Result<Option<String>, CmdError> {
    let host = crate::config::worker_name();
    let Some(text) = store.download_text(&kept_key(&host, resource)).await? else {
        return Ok(None);
    };
    let kept: Value = serde_json::from_str(&text).map_err(|error| {
        CmdError::declaration(format!(
            "{} is not a kept offer: {error}",
            kept_key(&host, resource)
        ))
    })?;
    Ok(kept["offer"].as_str().map(str::to_string))
}

fn refuse(what: &str) -> Result<(), CmdError> {
    Err(CmdError::refused(format!("HandToHuman {what}")))
}

#[async_trait::async_trait]
impl MarketAdapter for HandToHumanAdapter {
    async fn list(&self, offer: ListOffer, json: bool) -> Result<(), CmdError> {
        let resource = self.resource("list")?;
        let (kind, unit) = resource.kind()?;
        if offer.price_disk.is_some() || offer.price_min_bid.is_some() {
            return Err(CmdError::usage(
                "--price-disk and --price-min-bid are Vast.ai's prices; a HandToHuman offer is \
                 priced by --price-gpu, in USD per unit of its resource",
            ));
        }
        if !(offer.price_gpu.is_finite() && offer.price_gpu > 0.0) {
            return Err(CmdError::usage("--price-gpu must be a price above zero"));
        }
        let cents = (offer.price_gpu * 100.0).round() as i64;
        let units = offer.units.ok_or_else(|| {
            CmdError::usage(format!(
                "a HandToHuman offer states how many {unit}s it sells: pass --units"
            ))
        })?;
        let seller = seller()?;
        let host = crate::config::worker_name();
        let store = JobStorage::new().await?;
        if let Some(existing) = kept_offer(&store, resource).await? {
            return Err(CmdError::refused(format!(
                "{host} already offers {} on HandToHuman as {existing}; `stado market unlist \
                 --provider handtohuman --resource {}` withdraws it first",
                resource.word(),
                resource.word()
            )));
        }
        // The offer names the host, so an order's dispatch pins its work
        // back here (`pinned_host`); HandToHuman reads it from the path
        // given, which is this process's standard input, so nothing is
        // written to disk.
        let metadata = json!({"pinned_host": host, "offered_by": "stado"}).to_string();
        let title = format!("{} from {host}", resource.word());
        let answer = market_with(
            &[
                "offer",
                "create",
                &seller,
                "--kind",
                kind,
                "--title",
                &title,
                "--unit-price-cents",
                &cents.to_string(),
                "--currency",
                "usd",
                "--units",
                &units.to_string(),
                "--metadata",
                "/dev/stdin",
            ],
            Some(&metadata),
        )
        .await?;
        let id = answer["id"].as_str().ok_or_else(|| {
            CmdError::declaration(format!(
                "handtohuman market offer create answered no id: {answer}"
            ))
        })?;
        store
            .upload_text(
                &kept_key(&host, resource),
                &json!({"offer": id, "seller": seller, "listed_at": chrono::Utc::now().to_rfc3339()})
                    .to_string(),
            )
            .await?;
        crate::cli::print_answer(&answer, json)?;
        Ok(())
    }

    async fn unlist(&self, json: bool) -> Result<(), CmdError> {
        let resource = self.resource("unlist")?;
        let host = crate::config::worker_name();
        let store = JobStorage::new().await?;
        let Some(offer) = kept_offer(&store, resource).await? else {
            return Err(CmdError::missing(format!(
                "{host} offers no {} on HandToHuman",
                resource.word()
            )));
        };
        let answer = market(&["offer", "withdraw", &offer]).await?;
        store.delete_blob(&kept_key(&host, resource)).await?;
        crate::cli::print_answer(&answer, json)?;
        Ok(())
    }

    async fn status(&self, json: bool) -> Result<(), CmdError> {
        let resource = self.resource("status")?;
        let store = JobStorage::new().await?;
        let Some(offer) = kept_offer(&store, resource).await? else {
            return Err(CmdError::missing(format!(
                "{} offers no {} on HandToHuman",
                crate::config::worker_name(),
                resource.word()
            )));
        };
        let answer = market(&["offer", "show", &offer]).await?;
        crate::cli::print_answer(&answer, json)?;
        Ok(())
    }

    /// What the host's offer sold: every order HandToHuman holds against
    /// it, and what the accepted ones paid, in the order's currency.
    async fn earnings(&self, _target: &str, _since_days: u32, json: bool) -> Result<(), CmdError> {
        let resource = self.resource("earnings")?;
        let store = JobStorage::new().await?;
        let Some(offer) = kept_offer(&store, resource).await? else {
            return Err(CmdError::missing(format!(
                "{} offers no {} on HandToHuman, so it earned nothing there",
                crate::config::worker_name(),
                resource.word()
            )));
        };
        let orders = market(&["order", "list", "--offer", &offer]).await?;
        let rows = orders.as_array().cloned().unwrap_or_default();
        // Acceptance releases an order's escrow to the seller less the
        // platform fee; no other state has paid it.
        let paid: Vec<&Value> = rows
            .iter()
            .filter(|order| order["state"].as_str() == Some("accepted"))
            .collect();
        let mut by_currency: std::collections::BTreeMap<String, i64> = Default::default();
        for order in &paid {
            *by_currency
                .entry(order["currency"].as_str().unwrap_or("?").to_string())
                .or_default() += order["total_cents"].as_i64().unwrap_or(0);
        }
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "schema": "stado.handtohuman-earnings.v1",
                    "offer": offer,
                    "orders": rows,
                    "paid_cents": by_currency,
                }))?
            );
            return Ok(());
        }
        println!(
            "offer {offer}: {} order(s), {} paid",
            rows.len(),
            paid.len()
        );
        for (currency, cents) in &by_currency {
            println!(
                "  paid {:.2} {currency} before HandToHuman's platform fee",
                *cents as f64 / 100.0
            );
        }
        Ok(())
    }

    async fn maintenance(&self, _: u32, _: f64, _: &str, _: bool) -> Result<(), CmdError> {
        refuse(
            "holds no renter to notify: an order's work is a job on this host's queue, so take \
             the host out with `stado market unlist --provider handtohuman --resource …`",
        )
    }

    async fn measure(&self, _: &str, _: bool) -> Result<(), CmdError> {
        refuse(
            "publishes no market of prices: `handtohuman market offer list --kind compute` lists \
             the offers buyers see",
        )
    }

    async fn serve_or_rent(&self, _: ServeOrRent<'_>, _: bool) -> Result<(), CmdError> {
        refuse("states no rental price per board model; run serve-or-rent with --provider vast")
    }

    async fn readiness(&self, _: Option<String>, _: bool, json: bool) -> Result<(), CmdError> {
        let rows = self.verdicts().await;
        if json {
            println!("{}", serde_json::to_string_pretty(&rows)?);
        } else {
            for row in &rows {
                println!(
                    "{}: {}",
                    if row["ready"] == true {
                        "ready"
                    } else {
                        "not ready"
                    },
                    row["verdict"].as_str().unwrap_or("?")
                );
            }
        }
        match rows.iter().all(|row| row["ready"] == true) {
            true => Ok(()),
            false => Err(CmdError::silent(crate::cli::CLICK_ERROR_CODE)),
        }
    }

    async fn verdicts(&self) -> Vec<Value> {
        let row = match (
            seller(),
            market(&["offer", "list", "--kind", "compute"]).await,
        ) {
            (Err(error), _) => {
                json!({"provider": "handtohuman", "ready": false, "verdict": error.to_string()})
            }
            (Ok(_), Err(error)) => {
                json!({"provider": "handtohuman", "ready": false, "verdict": error.to_string()})
            }
            (Ok(seller), Ok(_)) => json!({
                "provider": "handtohuman",
                "ready": true,
                "verdict": format!("HandToHuman answers and this host offers as seller {seller}"),
            }),
        };
        vec![row]
    }

    async fn monitor(&self, _: &str, json: bool) -> Result<(), CmdError> {
        self.status(json).await
    }

    async fn follow(&self, _: Follow) -> Result<(), CmdError> {
        refuse(
            "orders are dispatched to this host's own queue, so there is no idle loop to follow: \
             list the offer once and withdraw it with unlist",
        )
    }
}
