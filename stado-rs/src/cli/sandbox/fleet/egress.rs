//! A fleet sandbox's declared egress: the host names it may reach over HTTP
//! and HTTPS, and nothing else.
//!
//! A sandbox with no declared host has no network at all (`--network none`).
//! With `--egress HOST` it is attached only to its own internal Docker
//! network (`docker network create --internal`), which has no route out of
//! the host, and the one container on both that network and the default
//! bridge is its egress proxy: tinyproxy, from an immutable image the
//! caller declares (`--egress-proxy-image`), configured to deny by default
//! and to pass only the declared host names, exactly (`FilterDefaultDeny
//! Yes`, `FilterType ere`, `FilterURLs Off`, one anchored pattern per host,
//! CONNECT only to HTTPS). The sandbox's `HTTP_PROXY` and `HTTPS_PROXY` name
//! that proxy; a program that ignores them, or speaks another protocol,
//! reaches nothing, since its network leads nowhere else. Stado changes no
//! firewall of the host: the boundary is Docker's internal network and the
//! proxy's filter. The proxy's log, `docker logs`, is what was asked and
//! refused.

use crate::cli::CmdError;

/// A host name a sandbox may reach: lowercase DNS labels of letters, digits
/// and hyphens, joined by dots.
pub(super) fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// The declared hosts as the programs carry them, one word each, or the
/// refusal of a malformed one.
pub(super) fn hosts(egress: &[String]) -> Result<String, CmdError> {
    for host in egress {
        if !valid_host(host) {
            return Err(CmdError::usage(format!(
                "--egress {host:?} is no host name: lowercase letters, digits and hyphens in dot-separated labels; no sandbox was created"
            )));
        }
    }
    Ok(egress.join(" "))
}

/// Shell functions for a sandbox's egress, once `name`, `egress` (the
/// declared hosts, space-separated, empty for none) and `proxy_image` are
/// set: `egress_names` names the network, proxy and its configuration and
/// sets `network_flags`, what the sandbox's `docker create` is given;
/// `build_proxy` makes them; `remove_egress` takes them away. tinyproxy
/// listens on every address of its own container, the port below is the one
/// its configuration names.
pub const EGRESS: &str = r#"
proxy_port=8888
egress_names() {
  egress_network=stado-sandbox-$name-egress
  proxy=stado-sandbox-$name-proxy
  proxy_config=stado-sandbox-$name-proxy-config
  proxy_url=http://$proxy:$proxy_port
  network_flags=(--network none)
  if [ -n "$egress" ]; then
    network_flags=(--network "$egress_network" --env "HTTP_PROXY=$proxy_url" --env "HTTPS_PROXY=$proxy_url" --env "http_proxy=$proxy_url" --env "https_proxy=$proxy_url")
  fi
}
egress_absent() {
  [ -z "$(docker ps --all --filter "name=^/$proxy$" --format '{{.Names}}')" ] || fail "egress proxy $proxy already exists; refusing to adopt it"
  [ -z "$(docker network ls --filter "name=^$egress_network$" --format '{{.Name}}')" ] || fail "network $egress_network already exists; refusing to adopt it"
  [ -z "$(docker volume ls --filter "name=^$proxy_config$" --format '{{.Name}}')" ] || fail "proxy configuration $proxy_config already exists; refusing to adopt it"
}
build_proxy() {
  [ -n "$egress" ] || return 0
  egress_absent
  docker pull "$proxy_image"
  docker network create --internal --label com.wisent.stado.sandbox=fleet "$egress_network"
  created_network=$egress_network
  docker volume create --label com.wisent.stado.sandbox=fleet "$proxy_config"
  created_config=$proxy_config
  printf 'Port %s\nFilterDefaultDeny Yes\nFilterType ere\nFilterURLs Off\nFilter "/etc/stado/filter"\nConnectPort 443\n' "$proxy_port" \
    | docker run --rm -i --runtime runsc --network none --cap-drop ALL --security-opt no-new-privileges \
      --mount "type=volume,source=$proxy_config,target=/c" --entrypoint /bin/sh "$proxy_image" -c 'cat > /c/tinyproxy.conf'
  for host in $egress; do printf '^%s$\n' "${host//./\\.}"; done \
    | docker run --rm -i --runtime runsc --network none --cap-drop ALL --security-opt no-new-privileges \
      --mount "type=volume,source=$proxy_config,target=/c" --entrypoint /bin/sh "$proxy_image" -c 'cat > /c/filter'
  created_proxy=$(docker create --name "$proxy" --label com.wisent.stado.sandbox=fleet-proxy \
    --label "com.wisent.stado.sandbox.proxy-of=$name" \
    --runtime runsc --read-only --cap-drop ALL --security-opt no-new-privileges \
    --mount "type=volume,source=$proxy_config,target=/etc/stado,readonly" \
    --tmpfs /tmp --tmpfs /var/run/tinyproxy --tmpfs /var/log/tinyproxy \
    --entrypoint tinyproxy "$proxy_image" -d -c /etc/stado/tinyproxy.conf)
  docker network connect "$egress_network" "$proxy"
  docker start "$proxy"
  [ "$(docker inspect --format '{{.State.Running}}' "$proxy")" = true ] || fail "egress proxy $proxy did not remain running; its log: $(docker logs "$proxy" 2>&1)"
}
remove_egress() {
  if [ -n "$(docker ps --all --filter "name=^/$proxy$" --format '{{.Names}}')" ]; then
    [ "$(docker inspect --format '{{index .Config.Labels "com.wisent.stado.sandbox.proxy-of"}}' "$proxy")" = "$name" ] || fail "container $proxy is not $name's egress proxy; nothing of it was removed"
    docker container rm --force "$proxy"
  fi
  if [ -n "$(docker volume ls --filter "name=^$proxy_config$" --format '{{.Name}}')" ]; then docker volume rm "$proxy_config"; fi
  if [ -n "$(docker network ls --filter "name=^$egress_network$" --format '{{.Name}}')" ]; then docker network rm "$egress_network"; fi
  [ -z "$(docker ps --all --filter "name=^/$proxy$" --format '{{.Names}}')" ] || fail 'egress proxy still exists after deletion'
  [ -z "$(docker network ls --filter "name=^$egress_network$" --format '{{.Name}}')" ] || fail 'egress network still exists after deletion'
}
"#;
