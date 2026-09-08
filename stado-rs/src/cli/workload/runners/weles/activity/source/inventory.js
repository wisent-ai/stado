const fs = require('node:fs');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');

const runLimit = Math.max(1, Number.parseInt(process.argv.at(-2), 10) || 40);
const apiPort = Number.parseInt(process.argv.at(-1), 10) || 8788;
const home = os.homedir();
const legacyWorkerRoot = path.join(home, '.local/share/weles-worker');
const managedServiceRoot = path.join(home, '.stado/services/weles-admission');
const managedWorkerRoot = path.join(managedServiceRoot, 'current');

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

const compareVersions = (left, right) => {
  const parts = (value) => String(value).split('.').map((piece) => Number.parseInt(piece, 10) || 0);
  const [a, b] = [parts(left), parts(right)];
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference !== 0) return difference;
  }
  return 0;
};

const releaseVersions = new Set();
const recordingSources = [];
const addRecordingSource = (release, platform, recordings, priority) => {
  if (typeof release !== 'string' || !release) return;
  try {
    if (!fs.statSync(recordings).isDirectory()) return;
  } catch {
    return;
  }
  releaseVersions.add(release);
  recordingSources.push({ release, platform, recordings, priority });
};
const addManagedRuntime = (runtime, platform, priority) => {
  const manifest = readJson(path.join(runtime, 'package.json'));
  const release = typeof manifest?.version === 'string' && manifest.version
    ? manifest.version
    : null;
  if (release) releaseVersions.add(release);
  addRecordingSource(release, platform, path.join(runtime, 'recordings'), priority);
};

// `current` is the active immutable coordinate. Count its release even before
// the first browser run creates a recordings directory.
addManagedRuntime(path.join(managedWorkerRoot, 'runtime'), 'managed', 2);

// Also report every immutable release Stado installed. The service store is
// digest-addressed (`sha256-*/<platform>/runtime`), not version-addressed, and
// tying release discovery to a recordings directory hid fresh installations
// until their first browser artifact existed.
try {
  for (const releaseEntry of fs.readdirSync(managedServiceRoot, { withFileTypes: true })) {
    if (!releaseEntry.isDirectory() || !releaseEntry.name.startsWith('sha256-')) continue;
    const releaseRoot = path.join(managedServiceRoot, releaseEntry.name);
    for (const platformEntry of fs.readdirSync(releaseRoot, { withFileTypes: true })) {
      if (!platformEntry.isDirectory()) continue;
      addManagedRuntime(
        path.join(releaseRoot, platformEntry.name, 'runtime'),
        platformEntry.name,
        1,
      );
    }
  }
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}

// Keep reporting recordings written by the retired per-version installer while
// hosts complete their cutover to the fleet-managed service.
try {
  for (const releaseEntry of fs.readdirSync(legacyWorkerRoot, { withFileTypes: true })) {
    if (!releaseEntry.isDirectory()) continue;
    const release = releaseEntry.name;
    const releaseRoot = path.join(legacyWorkerRoot, release);
    for (const platformEntry of fs.readdirSync(releaseRoot, { withFileTypes: true })) {
      if (!platformEntry.isDirectory()) continue;
      addRecordingSource(
        release,
        platformEntry.name,
        path.join(releaseRoot, platformEntry.name, 'recordings'),
        0,
      );
    }
  }
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}
const releases = [...releaseVersions].sort(compareVersions);

// The version marker names the release the retired activator staged. It can
// disagree with the active fleet-managed release and remains useful evidence
// that the old delivery path has not been removed from a host yet.
const releaseMarker = (() => {
  try {
    return fs.readFileSync(path.join(home, '.stado/files/weles-release-version'), 'utf8').trim() || null;
  } catch {
    return null;
  }
})();
