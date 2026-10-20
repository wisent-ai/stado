//! Retiring what release control left on a host for products whose release
//! policy no longer targets it.

use crate::release_agent::rollout::processes::inventory;
use crate::release_agent::rollout::serving::control;

/// Retire what release control left on this host for every product whose
/// policy no longer targets it: the proxy its state file owns, the release
/// processes it launched (only processes running out of the product's
/// release directory with the agent's launch marker — a pid from an old
/// state file may belong to anything by now), and its state and proxy files.
/// `targeted` names the products whose policy lists this host; `home` is the
/// account's home, under which every target keeps `.stado/release-state` and
/// `.stado/services/<product>`. Returns one line per product acted on.
pub(crate) async fn retire_untargeted(
    home: &str,
    targeted: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    let state_dir = std::path::Path::new(home).join(".stado/release-state");
    // No release-state directory means release control never left anything
    // here. Any other failure is reported: read as empty, a product taken off
    // this host kept its proxy and processes running with nothing said.
    let entries = match std::fs::read_dir(&state_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            return vec![format!(
                "release state {} could not be listed; nothing was retired: {error}",
                state_dir.display()
            )]
        }
    };
    let mut lines = Vec::new();
    let mut products: Vec<String> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                lines.push(format!(
                    "an entry of release state {} could not be read: {error}",
                    state_dir.display()
                ));
                continue;
            }
        };
        match entry.file_name().into_string() {
            Ok(name) => products.extend(
                name.strip_suffix("-proxy.json")
                    .map(str::to_string)
                    .filter(|product| !targeted.contains(product)),
            ),
            // Release control names every file it writes after a product,
            // and product names are text; a name that is not is no file of
            // release control's, and is said rather than passed over.
            Err(raw) => lines.push(format!(
                "release state {} holds an entry whose name is not text, {raw:?}; it names no \
                 product and was left alone",
                state_dir.display()
            )),
        }
    }
    products.sort();
    for product in products {
        let proxy = state_dir.join(format!("{product}-proxy.json"));
        if let Err(error) = control::retire(Some(home), &proxy).await {
            lines.push(format!(
                "{product} is no longer released to this host, but its release proxy could \
                 not be retired: {error}"
            ));
            continue;
        }
        let install_root = format!("{home}/.stado/services/{product}");
        let processes = match inventory::release_processes(&install_root) {
            Ok(processes) => processes,
            Err(error) => {
                lines.push(format!(
                    "{product} is no longer released to this host and its release proxy was \
                     retired, but its release processes could not be listed: {error}"
                ));
                continue;
            }
        };
        let spawned: Vec<i32> = processes
            .iter()
            .filter(|process| process.agent_spawned)
            .map(|process| process.process_group)
            .collect();
        let mut ended = Vec::new();
        for process in &processes {
            if process.agent_spawned || spawned.contains(&process.process_group) {
                match inventory::terminate(process) {
                    Ok(()) => ended.push(format!(
                        "{} pid {} port {:?}",
                        process.version, process.pid, process.port
                    )),
                    Err(failed) => lines.push(format!("{product}: {failed}")),
                }
            }
        }
        if !ended.is_empty() {
            // Removed on the pass that finds nothing left running, so a
            // process that ignored this SIGTERM is found and signalled again.
            lines.push(format!(
                "{product} is no longer released to this host: retired its release proxy and \
                 ended {}",
                ended.join(", ")
            ));
            continue;
        }
        let left: Vec<String> = [proxy.clone(), state_dir.join(format!("{product}.json"))]
            .iter()
            .filter_map(|path| crate::primitives::staging::remove(path))
            .collect();
        lines.push(if left.is_empty() {
            format!(
                "{product} is no longer released to this host: nothing of it runs here; its \
                 release state was removed"
            )
        } else {
            format!(
                "{product} is no longer released to this host: nothing of it runs here, but its \
                 release state stays: {}",
                left.join("; ")
            )
        });
    }
    lines
}
