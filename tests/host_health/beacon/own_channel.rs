//! A host's commands against its own registry entry run here, never over SSH
//! to itself, by every identity the registry gives the entry: its name, a
//! hostname alias, or the host of a declared SSH path - the same rule the
//! host process finds its own entry by. One isolated deployment per shape:
//! `config init` seeds a registry naming this machine; the entry is rewritten
//! to carry only the identity under test and imported; `stado host link
//! <target> --json` reports the connection the ping ran on, which must be
//! `local`, and the kernel's own hostname is what the entry is matched to.
mod deployment;

use deployment::Deployment;
use serde_json::{json, Value};
use std::fs;
use std::process::Command;

/// This machine's hostname as the kernel answers it, lowercased, with the
/// mDNS suffix the registry does not spell.
fn own_hostname_stem() -> String {
    let hostname = Command::new("hostname").output().unwrap();
    String::from_utf8(hostname.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
        .trim_end_matches(".local")
        .to_string()
}

/// Rewrite the seeded target as `shape` declares this machine, import it,
/// and answer the name `host link` is asked for.
fn declare(deployment: &mut Deployment, shape: &str) -> String {
    let registry_path = deployment.store().join("registry.json");
    let mut registry: Value = serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
    let stem = own_hostname_stem();
    let target = registry["targets"]
        .as_array_mut()
        .and_then(|targets| targets.first_mut())
        .expect("config init declares this machine");
    target.as_object_mut().unwrap().remove("hostnames");
    target.as_object_mut().unwrap().remove("ssh");
    let name = match shape {
        "name" => {
            target["name"] = json!(stem);
            stem.clone()
        }
        "alias" => {
            target["name"] = json!("declared-by-alias");
            target["hostnames"] = json!([format!("{stem}.local")]);
            "declared-by-alias".to_string()
        }
        "ssh" => {
            target["name"] = json!("declared-by-ssh-host");
            target["ssh"] = json!(format!("operator@{stem}"));
            "declared-by-ssh-host".to_string()
        }
        other => panic!("no such shape: {other}"),
    };
    let written = deployment.root.join(format!("registry-{shape}.json"));
    fs::write(&written, serde_json::to_vec_pretty(&registry).unwrap()).unwrap();
    deployment.cli(&["registry", "import", &written.to_string_lossy()]);
    name
}

#[test]
fn a_host_reaches_its_own_entry_locally_by_name_alias_or_ssh_host() {
    for shape in ["name", "alias", "ssh"] {
        let mut deployment = Deployment::start();
        let name = declare(&mut deployment, shape);
        let link: Value = serde_json::from_str(&deployment.cli(&["host", "link", &name, "--json"]))
            .expect("host link answers JSON");
        assert_eq!(
            link["selected_connection"], "local",
            "the entry declared by {shape} was not reached locally: {link}"
        );
        assert_eq!(link["ssh_reachable"], true, "{link}");
        deployment.report["shape"] = json!(shape);
        deployment.report["link"] = link;
        deployment.pass();
    }
}
