
// Weles API requests keep their process result outside a release runtime so an
// update cannot erase it. Fold those durable records into the live recording
// inventory: a cleaned recording loses its artifact counts, not the fact that
// the run happened or how its process ended.
const detachedRoot = path.join(home, '.stado/weles-detached-runs');
try {
  for (const entry of fs.readdirSync(detachedRoot, { withFileTypes: true })) {
    if (!entry.isFile() || !entry.name.endsWith('.json')) continue;
    const file = path.join(detachedRoot, entry.name);
    const document = readJson(file);
    if (!document || typeof document !== 'object') continue;
    const stat = fs.statSync(file);
    const fallbackId = entry.name.slice(0, -'.json'.length);
    const id = typeof document.run_id === 'string' && document.run_id
      ? document.run_id
      : fallbackId;
    const action = typeof document.action === 'string' && document.action
      ? document.action
      : null;

    let status = 'recorded';
    if (document.status === 'running' || document.ok === null) status = 'running';
    else if (document.ok === true) status = 'succeeded';
    else if (document.ok === false || document.status === 'failed') status = 'failed';

    const resultCandidates = [
      document.result,
      document.result && typeof document.result === 'object' ? document.result.result : null,
    ];
    let result = null;
    for (const candidate of resultCandidates) {
      if (!candidate || typeof candidate !== 'object') continue;
      const healthy = typeof candidate.healthy === 'boolean' ? candidate.healthy : null;
      const signal = typeof candidate.signal === 'string' && candidate.signal ? candidate.signal : null;
      if (healthy !== null || signal) {
        result = {
          healthy,
          signal,
          recorded_at: typeof candidate.ts === 'string'
            ? candidate.ts
            : (typeof document.completed_at === 'string' ? document.completed_at : null),
        };
        break;
      }
    }

    const release = typeof document.release_version === 'string' && document.release_version
      ? document.release_version
      : null;
    const durable = {
      id,
      release,
      platform: process.platform,
      action,
      status,
      started_at: typeof document.started_at === 'string'
        ? document.started_at
        : isoOrNull(stat.birthtimeMs),
      completed_at: typeof document.completed_at === 'string' ? document.completed_at : null,
      updated_at: isoOrNull(stat.mtimeMs),
      artifact_counts: { screenshots: 0, pages: 0, videos: 0, logs: 0, records: 0, other: 0 },
      artifact_bytes: 0,
      cost_usd: null,
      result,
      uploaded: false,
      upload_proof: null,
    };
    const live = describedById.get(id);
    describedById.set(id, live
      ? {
          ...live,
          action: live.action ?? durable.action,
          status: durable.status === 'recorded' ? live.status : durable.status,
          started_at: live.started_at ?? durable.started_at,
          completed_at: durable.completed_at ?? live.completed_at,
          updated_at: durable.updated_at ?? live.updated_at,
          result: durable.result ?? live.result,
        }
      : durable);
  }
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}

const allDescribed = [...describedById.values()].sort(
  (left, right) => (Date.parse(right.updated_at ?? '') || 0) - (Date.parse(left.updated_at ?? '') || 0),
);
const runTotal = allDescribed.length;
const described = allDescribed.slice(0, runLimit);

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
