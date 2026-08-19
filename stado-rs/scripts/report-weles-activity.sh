#!/bin/bash
# Report what a Weles worker host is doing, as one JSON document on stdout.
#
# Travels inside the stado binary and runs over the fixed-script channel: there
# is nothing to install on the host and nothing left behind after the read.
#
# Recordings hold page DOM, console output, HAR bodies, personas and proxy
# identities. None of that is emitted. What leaves the host is counts,
# timestamps, run identifiers, artifact sizes, cost, and the pass/fail flag a
# trajectory wrote about itself — the fields a remote operator view needs to
# name a run and say how it ended.
set -euo pipefail

if [ -x /opt/homebrew/bin/node ]; then
  node=/opt/homebrew/bin/node
elif [ -x /usr/local/bin/node ]; then
  node=/usr/local/bin/node
else
  printf '%s\n' 'Node.js is unavailable on this host' >&2
  exit 69
fi

limit=${WELES_ACTIVITY_RUN_LIMIT:-40}
port=${WELES_API_PORT:-8788}

"$node" - "$limit" "$port" <<'NODE'
const fs = require('node:fs');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');

const runLimit = Math.max(1, Number.parseInt(process.argv.at(-2), 10) || 40);
const apiPort = Number.parseInt(process.argv.at(-1), 10) || 8788;
const home = os.homedir();
const workerRoot = path.join(home, '.local/share/weles-worker');

const hostname = String(os.hostname()).trim().toLowerCase().replace(/\.+$/, '');
const shortHostname = hostname.endsWith('.local') ? hostname.slice(0, -'.local'.length) : hostname;

const isoOrNull = (value) => {
  const time = Number(value);
  return Number.isFinite(time) && time > 0 ? new Date(time).toISOString() : null;
};

const readJson = (file) => {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return null;
  }
};

// The version marker names the release the activator staged; the directories say
// which releases actually ran here. The two disagree while a deploy is mid
// flight, and a report that carried only one of them would hide that.
const releaseMarker = (() => {
  try {
    return fs.readFileSync(path.join(home, '.stado/files/weles-release-version'), 'utf8').trim() || null;
  } catch {
    return null;
  }
})();

const compareVersions = (left, right) => {
  const parts = (value) => String(value).split('.').map((piece) => Number.parseInt(piece, 10) || 0);
  const [a, b] = [parts(left), parts(right)];
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference !== 0) return difference;
  }
  return 0;
};

let releases = [];
try {
  releases = fs
    .readdirSync(workerRoot, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .sort(compareVersions);
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}

const ARTIFACT_CLASSES = [
  ['screenshots', /\.png$/i],
  ['pages', /\.html$/i],
  ['videos', /\.webm$/i],
  ['logs', /\.(log|ndjson)$/i],
  ['records', /\.json$|\.jsonl$|\.har$/i],
];

const classify = (name) => {
  for (const [label, pattern] of ARTIFACT_CLASSES) {
    if (pattern.test(name)) return label;
  }
  return 'other';
};

const RUNNING_WINDOW_MS = 180_000;

const describeRun = (release, platform, runDirectory) => {
  const stat = fs.statSync(runDirectory);
  const counts = { screenshots: 0, pages: 0, videos: 0, logs: 0, records: 0, other: 0 };
  let bytes = 0;
  let action = null;
  let resultOk = null;
  let startedAt = null;
  let completedAt = null;

  const walk = (directory, depth) => {
    let entries = [];
    try {
      entries = fs.readdirSync(directory, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      const full = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        // The one directory directly under a run is the action that produced it.
        if (depth === 0 && !action) action = entry.name;
        if (depth < 4) walk(full, depth + 1);
        continue;
      }
      if (!entry.isFile()) continue;
      counts[classify(entry.name)] += 1;
      try {
        bytes += fs.statSync(full).size;
      } catch {
        // A file rotated away mid-walk is not worth failing the report over.
      }
      if (/result\.json$/i.test(entry.name)) {
        const document = readJson(full);
        if (document && typeof document.ok === 'boolean') resultOk = document.ok;
        if (typeof document?.completed_at === 'string') completedAt = document.completed_at;
      } else if (entry.name === 'session_meta.json') {
        const document = readJson(full);
        if (typeof document?.started_at === 'string') startedAt = document.started_at;
      }
    }
  };
  walk(runDirectory, 0);

  const uploaded = fs.existsSync(path.join(runDirectory, '.uploaded.json'));
  const costs = readJson(path.join(path.dirname(runDirectory), '_costs', `${path.basename(runDirectory)}.json`));
  const isFresh = Date.now() - stat.mtimeMs < RUNNING_WINDOW_MS;

  let status = 'recorded';
  if (resultOk === true) status = 'succeeded';
  else if (resultOk === false) status = 'failed';
  else if (isFresh) status = 'running';

  return {
    id: path.basename(runDirectory),
    release,
    platform,
    action,
    status,
    started_at: startedAt ?? isoOrNull(stat.birthtimeMs),
    completed_at: completedAt,
    updated_at: isoOrNull(stat.mtimeMs),
    artifact_counts: counts,
    artifact_bytes: bytes,
    cost_usd: typeof costs?.cost_usd === 'number' ? costs.cost_usd : null,
    uploaded,
  };
};

const runs = [];
let runTotal = 0;
for (const release of releases) {
  const releaseRoot = path.join(workerRoot, release);
  let platforms = [];
  try {
    platforms = fs
      .readdirSync(releaseRoot, { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name);
  } catch {
    continue;
  }
  for (const platform of platforms) {
    const recordings = path.join(releaseRoot, platform, 'recordings');
    let entries = [];
    try {
      entries = fs.readdirSync(recordings, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      // `_costs` is the sidecar ledger of the runs beside it, not a run.
      if (!entry.isDirectory() || entry.name === '_costs') continue;
      runTotal += 1;
      runs.push({ release, platform, directory: path.join(recordings, entry.name) });
    }
  }
}

runs.sort((left, right) => {
  const time = (row) => {
    try {
      return fs.statSync(row.directory).mtimeMs;
    } catch {
      return 0;
    }
  };
  return time(right) - time(left);
});

const described = runs.slice(0, runLimit).map((row) => describeRun(row.release, row.platform, row.directory));

const probePort = (port) =>
  new Promise((resolve) => {
    const socket = net.createConnection({ host: '127.0.0.1', port });
    const finish = (listening) => {
      socket.destroy();
      resolve(listening);
    };
    socket.setTimeout(1500);
    socket.once('connect', () => finish(true));
    socket.once('timeout', () => finish(false));
    socket.once('error', () => finish(false));
  });

probePort(apiPort).then((listening) => {
  const document = {
    schema_version: 1,
    host: shortHostname || hostname,
    hostname,
    generated_at: new Date().toISOString(),
    worker: {
      staged_release: releaseMarker,
      installed_releases: releases,
      newest_release: releases.at(-1) ?? null,
    },
    api: {
      endpoint: `http://127.0.0.1:${apiPort}`,
      listening,
    },
    run_total: runTotal,
    runs: described,
  };
  process.stdout.write(`STADO-WELES-ACTIVITY ${JSON.stringify(document)}\n`);
});
NODE
