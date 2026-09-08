
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
  let resultHealthy = null;
  let resultSignal = null;
  let resultAt = null;
  let uploadProof = null;
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
      } else if (entry.name === 'ban_signal.json') {
        const document = readJson(full);
        if (typeof document?.healthy === 'boolean') resultHealthy = document.healthy;
        if (typeof document?.signal === 'string' && document.signal) resultSignal = document.signal;
        if (typeof document?.ts === 'string') resultAt = document.ts;
      } else if (entry.name === '.uploaded.json') {
        const document = readJson(full);
        if (typeof document?.sha256 === 'string' && typeof document?.destination === 'string') {
          uploadProof = { sha256: document.sha256, destination: document.destination };
        }
      } else if (entry.name === 'session_meta.json') {
        const document = readJson(full);
        if (typeof document?.started_at === 'string') startedAt = document.started_at;
      }
    }
  };
  walk(runDirectory, 0);

  if (!uploadProof) {
    const document = readJson(path.join(runDirectory, '.uploaded.json'));
    if (typeof document?.sha256 === 'string' && typeof document?.destination === 'string') {
      uploadProof = { sha256: document.sha256, destination: document.destination };
    }
  }
  const costs = readJson(path.join(path.dirname(runDirectory), '_costs', `${path.basename(runDirectory)}.json`));
  const isFresh = Date.now() - stat.mtimeMs < RUNNING_WINDOW_MS;

  let status = 'recorded';
  if (resultHealthy === true || resultOk === true) status = 'succeeded';
  else if (resultHealthy === false || resultOk === false || resultSignal) status = 'failed';
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
    result: resultHealthy !== null || resultSignal
      ? { healthy: resultHealthy, signal: resultSignal, recorded_at: resultAt }
      : null,
    uploaded: uploadProof !== null,
    upload_proof: uploadProof,
  };
};

const runsById = new Map();
for (const source of recordingSources) {
  let entries = [];
  try {
    entries = fs.readdirSync(source.recordings, { withFileTypes: true });
  } catch {
    continue;
  }
  for (const entry of entries) {
    // `_costs` is the sidecar ledger of the runs beside it, not a run.
    if (!entry.isDirectory() || entry.name === '_costs') continue;
    const candidate = {
      release: source.release,
      platform: source.platform,
      directory: path.join(source.recordings, entry.name),
      priority: source.priority,
    };
    const existing = runsById.get(entry.name);
    if (!existing || candidate.priority > existing.priority) runsById.set(entry.name, candidate);
  }
}
const runs = [...runsById.values()];
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

const describedById = new Map();
for (const row of runs) {
  const summary = describeRun(row.release, row.platform, row.directory);
  describedById.set(summary.id, summary);
}
