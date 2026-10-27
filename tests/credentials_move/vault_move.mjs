import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { closeSync, constants, createReadStream, lstatSync, mkdirSync, mkdtempSync, openSync, readFileSync, readlinkSync, realpathSync, renameSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real relocation qualification on two isolated homes reached by genuine host
// channels. VAULT_MOVE_FIXTURE names a prepared local fixture, never the
// operator's fleet. Both homes and every mutable path must be below .build.
// Fixture fields: source and destination each carry host, home, vault,
// keyring, owner_uid and unit_file. The controller runs
// in source.home with the fixture's local store. ssh_key names its dedicated
// host-channel key. sentinel names an existing item; sentinel_fields contains
// its known fixture values, checked against actual decrypted field digests.
// Each keyring must hold the other owner's public key; the source vault must
// register the destination recipient. STADO_BIN and SKARBIEC_BIN name genuine
// same-platform product binaries.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const build = realpathSync(join(root, '.build'));
const output = mkdtempSync(join(build, 'credentials-vault-move-'));
const report = { started_at: new Date().toISOString(), commands: [], result: 'blocked' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const succeeded = answer => answer.status !== null && !answer.status;
const required = name => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required for the isolated real vault-move journey`);
  return value;
};
function insideBuild(path) {
  assert.ok(isAbsolute(path), `fixture path must be absolute: ${path}`);
  const suffix = relative(build, resolve(path));
  assert.ok(suffix && !suffix.startsWith('..') && !isAbsolute(suffix),
    `fixture path must be inside ${build}: ${path}`);
  return path;
}
function existing(path) {
  insideBuild(path);
  return insideBuild(realpathSync(path));
}
function run(program, args, env) {
  const answer = spawnSync(program, args, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(succeeded(answer), answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function refused(answer, sentence) {
  assert.ok(answer.status !== null && answer.status, `expected refusal: ${answer.stdout}`);
  assert.ok(answer.stderr?.includes(sentence), `expected ${sentence}: ${answer.stderr}`);
}
async function fileDigest(path) {
  const hash = createHash('sha256');
  for await (const bytes of createReadStream(path)) hash.update(bytes);
  return hash.digest('hex');
}
async function recordSource() {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD'], process.env));
  const path = join(output, 'source.patch');
  const descriptor = openSync(path, 'wx', constants.S_IRUSR | constants.S_IWUSR);
  const args = ['diff', '--binary', 'HEAD'];
  let answer;
  try {
    answer = spawnSync('git', args, {
      cwd: root, env: process.env, encoding: 'utf8',
      stdio: ['ignore', descriptor, 'pipe'],
    });
  } finally {
    closeSync(descriptor);
  }
  report.commands.push({
    program: 'git', arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message,
    stdout_file: path, stderr: answer.stderr,
  });
  assert.ok(succeeded(answer), answer.stderr || answer.error?.message || answer.signal);
  report.source_patch = { file: path, sha256: await fileDigest(path) };
  const untracked = run('git', ['ls-files', '--others', '--exclude-standard', '-z'], process.env);
  assert.ok(succeeded(untracked), untracked.stderr || untracked.error?.message);
  report.untracked_sources = [];
  for (const name of untracked.stdout.split('\0').filter(name => name !== '')) {
    const path = join(root, name);
    const metadata = lstatSync(path);
    if (metadata.isSymbolicLink()) {
      report.untracked_sources.push({ path: name, link: readlinkSync(path) });
    } else {
      assert.ok(metadata.isFile(), `untracked source is not a regular file: ${name}`);
      report.untracked_sources.push({
        path: name, mode: metadata.mode, bytes: metadata.size, sha256: await fileDigest(path),
      });
    }
  }
}
const document = path => JSON.parse(readFileSync(path, 'utf8'));
let fixture;
let stado;
let skarbiec;
let environment;
let heldUnit;
let restoreRequired = false;
let qualified = false;
const invoke = args => run(stado, args, environment);
const json = args => JSON.parse(success(invoke(args)));
const move = (host, uid, apply) => invoke([
  'credentials', 'vault', 'move', '--to', host, '--owner-uid', uid,
  ...(apply ? ['--apply'] : []), '--json',
]);
function configuration(host) {
  return json(['host', 'config', 'show', host, '--json']);
}
function fields(host) {
  const observed = json(['credentials', 'item', 'show', '--host', host, fixture.sentinel, '--json']).fields;
  assert.ok(Array.isArray(observed), 'the real item read returns field summaries');
  return observed.map(({ name, length, sha256 }) => ({ name, length, sha256 }))
    .sort((left, right) => left.name.localeCompare(right.name));
}
try {
  await recordSource();
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  stado = realpathSync(required('STADO_BIN'));
  skarbiec = realpathSync(required('SKARBIEC_BIN'));
  fixture = document(existing(required('VAULT_MOVE_FIXTURE')));
  for (const host of [fixture.source, fixture.destination]) {
    existing(host.home);
    existing(host.keyring);
    insideBuild(host.vault);
    assert.equal(dirname(host.vault), join(host.home, '.stado'));
    assert.ok(host.host && host.owner_uid, 'each isolated home names a registry host and owner');
    existing(dirname(host.vault));
    const held = lstatSync(host.vault, { throwIfNoEntry: false });
    if (held) {
      assert.ok(!held.isSymbolicLink(), `fixture vault cannot be a symlink: ${host.vault}`);
      existing(host.vault);
    }
  }
  assert.notEqual(fixture.source.host, fixture.destination.host);
  assert.notEqual(fixture.source.home, fixture.destination.home);
  const unit = existing(fixture.destination.unit_file);
  const sourceUnit = existing(fixture.source.unit_file);
  existing(fixture.ssh_key);
  existing(join(fixture.source.home, 'store'));
  const config = existing(join(fixture.source.home, '.stado/config.json'));
  const scratch = join(output, 'tmp');
  mkdirSync(scratch);
  environment = {
    PATH: required('PATH'), HOME: fixture.source.home, STADO_CONFIG: config,
    GNUPGHOME: fixture.source.keyring, WC_STORAGE_BACKEND: 'local',
    WC_LOCAL_STORAGE_PATH: join(fixture.source.home, 'store'),
    STADO_HOST_SSH_KEY_FILE: fixture.ssh_key, TMPDIR: scratch,
  };
  report.binaries = {
    stado: { path: stado, sha256: digest(readFileSync(stado)), version: success(invoke(['--version'])) },
    skarbiec: { path: skarbiec, sha256: digest(readFileSync(skarbiec)) },
  };
  for (const host of [fixture.source, fixture.destination]) {
    const actual = configuration(host.host);
    assert.equal(dirname(actual.file), join(host.home, '.stado'),
      'the real host channel must resolve the isolated home, not an operator configuration');
  }
  const source = fixture.source;
  const destination = fixture.destination;
  assert.equal(configuration(source.host).resolved.skarbiec_vault_file, source.vault);
  const original = document(existing(source.vault));
  assert.equal(original.owner, source.owner_uid);
  assert.ok(original.recipients[destination.owner_uid], 'fixture registers the destination public key');
  assert.ok(fixture.sentinel_fields && Object.keys(fixture.sentinel_fields).length,
    'fixture must state the sentinel fields whose real decryption is checked');
  const expectedFields = Object.entries(fixture.sentinel_fields)
    .map(([name, value]) => {
      assert.equal(typeof value, 'string', `fixture field ${name} must be a string`);
      return { name, length: Buffer.byteLength(value), sha256: digest(Buffer.from(value)) };
    })
    .sort((left, right) => left.name.localeCompare(right.name));
  assert.deepEqual(fields(source.host), expectedFields);
  const inventory = Object.keys(original.items).sort();
  const before = digest(readFileSync(source.vault));
  const unchanged = () => assert.equal(digest(readFileSync(source.vault)), before);
  qualified = true;
  report.result = 'failed';

  const absent = `vault-move-absent-${randomUUID()}`;
  refused(move(destination.host, absent, true), `holds no secret half for ${absent}`);
  unchanged();
  const held = `${unit}.held-by-journey`;
  assert.ok(!lstatSync(held, { throwIfNoEntry: false }), `fixture backup already exists: ${held}`);
  renameSync(unit, held);
  heldUnit = { unit, held };
  refused(move(destination.host, destination.owner_uid, true), `destination unit file is missing: ${unit}`);
  unchanged();
  renameSync(held, unit);
  heldUnit = undefined;

  // Reproduce the intermediate state with Skarbiec, then ask Stado to finish
  // it. A repeated owner rotate would be refused by the real Skarbiec.
  restoreRequired = true;
  success(run(skarbiec, ['owner', 'rotate', destination.owner_uid], {
    ...environment, SKARBIEC_VAULT_FILE: source.vault,
  }));
  assert.equal(document(source.vault).owner, destination.owner_uid);
  const rotated = digest(readFileSync(source.vault));
  const heldSource = `${sourceUnit}.held-by-journey`;
  assert.ok(!lstatSync(heldSource, { throwIfNoEntry: false }), `fixture backup already exists: ${heldSource}`);
  renameSync(sourceUnit, heldSource);
  heldUnit = { unit: sourceUnit, held: heldSource };
  // The placement check refuses the missing source unit before the command
  // registers or rotates anything, and the read-only pass reports it.
  const unready = JSON.parse(success(move(destination.host, destination.owner_uid, false)));
  assert.ok(unready.placement_check.refused?.includes(`source unit file is missing`),
    JSON.stringify(unready.placement_check));
  refused(move(destination.host, destination.owner_uid, true),
    'would be refused, so nothing was registered or rotated');
  assert.equal(digest(readFileSync(source.vault)), rotated,
    'a failed continuation neither rotates nor restores the already-selected owner');
  renameSync(heldSource, sourceUnit);
  heldUnit = undefined;
  const resumed = JSON.parse(success(move(destination.host, destination.owner_uid, false)));
  assert.equal(resumed.rotation_required, false);
  assert.equal(resumed.placement_check.status, 'would_move', JSON.stringify(resumed.placement_check));
  report.resumed = resumed;
  const receipt = JSON.parse(success(move(destination.host, destination.owner_uid, true)));
  report.move = receipt;
  const after = document(existing(destination.vault));
  assert.equal(after.owner, destination.owner_uid);
  assert.deepEqual(Object.keys(after.items).sort(), inventory);
  assert.deepEqual(fields(destination.host), expectedFields, 'the destination decrypts the same real fields');
  assert.equal(configuration(destination.host).resolved.skarbiec_vault_file, destination.vault);
  assert.ok(!configuration(source.host).resolved.skarbiec_vault_file);
  assert.equal(json(['registry', 'pull']).service_directory.services.skarbiec.active_host, destination.host);
  refused(move(destination.host, destination.owner_uid, true), 'already holds the fleet vault');
  assert.equal(document(destination.vault).owner, destination.owner_uid);

  report.return_move = JSON.parse(success(move(source.host, source.owner_uid, true)));
  assert.equal(document(source.vault).owner, source.owner_uid);
  assert.deepEqual(fields(source.host), expectedFields);
  assert.equal(json(['registry', 'pull']).service_directory.services.skarbiec.active_host, source.host);
  restoreRequired = false;
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  if (!qualified) report.result = 'blocked';
} finally {
  if (heldUnit) {
    try { renameSync(heldUnit.held, heldUnit.unit); }
    catch (error) { report.unit_restore_error = String(error); report.result = 'failed'; }
  }
  if (restoreRequired) {
    try {
      const source = fixture.source;
      const active = json(['registry', 'pull']).service_directory.services.skarbiec.active_host;
      if (active === fixture.destination.host) {
        report.fixture_restore = JSON.parse(success(move(source.host, source.owner_uid, true)));
      } else {
        assert.equal(active, source.host, 'restore refuses an unexpected active host');
        if (document(source.vault).owner !== source.owner_uid) {
          report.fixture_owner_restore = JSON.parse(success(run(skarbiec,
            ['owner', 'rotate', source.owner_uid], {
              ...environment, HOME: fixture.destination.home,
              GNUPGHOME: fixture.destination.keyring, SKARBIEC_VAULT_FILE: source.vault,
            })));
        }
      }
      assert.equal(document(source.vault).owner, source.owner_uid, 'fixture owner is restored');
      fields(source.host);
    } catch (error) {
      report.fixture_restore_error = error.stack || String(error);
      report.result = 'failed';
    }
  }
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`,
    { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (report.result !== 'passed') throw new Error(`vault-move qualification ${report.result}; report in ${output}`);
