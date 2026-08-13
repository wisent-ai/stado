#!/bin/sh
# Reconcile control-plane credential routing in an existing installation.
set -eu
umask 077

stado_bin=${STADO_BIN:-$HOME/.stado/bin/stado}
config_file=${STADO_CONFIG:-$HOME/.config/stado/config.json}
node_bin=${NODE_BIN:-/opt/homebrew/bin/node}
for required in "$stado_bin" "$config_file" "$node_bin"; do
  [ -f "$required" ] || {
    printf '%s\n' "missing control-plane configuration input: $required" >&2
    exit 1
  }
done

"$node_bin" -e '
  const fs = require("node:fs");
  const path = process.argv[1];
  const config = JSON.parse(fs.readFileSync(path, "utf8"));
  config.credentials ??= {};
  config.credentials.admin = {
    consumer: "stado-control-plane",
    token_file: "~/.stado/control-plane-skarbiec-token",
  };
  if (config.integration?.clients) {
    const enterprise = config.integration.clients["wisent-enterprise"];
    config.integration.clients = enterprise ? { "wisent-enterprise": enterprise } : {};
  }
  if (config.integration?.providers) {
    delete config.integration.providers.backend;
    delete config.integration.providers.most;
    if (Object.keys(config.integration.providers).length === 0) {
      delete config.integration.providers;
    }
  }
  const temporary = `${path}.tmp-${process.pid}`;
  const mode = fs.statSync(path).mode & 0o777;
  fs.writeFileSync(temporary, `${JSON.stringify(config, null, 2)}\n`, { mode });
  fs.renameSync(temporary, path);
' "$config_file"

"$stado_bin" config validate >/dev/null
printf '%s\n' 'configured credentials.admin for stado-control-plane'
