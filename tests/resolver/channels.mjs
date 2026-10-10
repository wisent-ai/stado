// Real CLI and live resolver publication; filesystem refusals use isolated paths.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} must identify the real resolver fixture`);
  return value;
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = resolve(required('STADO_BIN'));
const revision = required('STADO_TEST_SOURCE_REVISION');
const bind = required('STADO_TEST_CHANNEL_BIND');
const consumer = required('STADO_TEST_CHANNEL_CONSUMER');
const area = join(root, '.build/resolver-channels', randomUUID());
mkdirSync(area, { recursive: true });
const digest = path => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: digest(binary),
  testSha256: digest(fileURLToPath(import.meta.url)), bind, consumer, commands: [], verdict: 'failed' };
function invoke(program, args, env = process.env) {
  const result = spawnSync(program, args, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ program, args, cwd: root, stateFile: env.STADO_RESOLVER_STATE_FILE,
    status: result.status, signal: result.signal, error: result.error?.message,
    stdout: result.stdout, stderr: result.stderr });
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  return result;
}
function succeeds(result) {
  assert.ok(result.status !== null && !result.status, result.stderr);
  return result.stdout;
}
function refused(result, path) {
  assert.ok(result.status !== null && result.status, 'an unavailable publication must refuse');
  assert.ok(`${result.stdout}\n${result.stderr}`.includes(path), 'the actual publication path must be retained');
}
try {
  assert.equal(succeeds(invoke('git', ['rev-parse', 'HEAD'])).trim(), revision);
  succeeds(invoke('git', ['diff', '--exit-code', 'HEAD']));
  const live = JSON.parse(succeeds(invoke(binary, [
    'resolver', 'channels', '--bind', bind, '--consumer', consumer, '--json',
  ])));
  assert.equal(live.health_observed, false);
  assert.equal(live.bind, bind);
  assert.equal(live.consumer, consumer);
  assert.ok(live.channels.some(channel => channel.bind === bind && channel.consumer === consumer),
    'the real resolver must currently publish the declared waiting connection');
  assert.ok(live.channels.every(channel => channel.bind === bind && channel.consumer === consumer),
    'both filters must restrict the actual channel publication');
  report.live = live;
  for (const kind of ['missing', 'directory', 'malformed']) {
    const directory = join(area, kind);
    mkdirSync(directory, { recursive: true });
    const publication = join(directory, 'resolver-waiting.json');
    if (kind === 'directory') mkdirSync(publication);
    if (kind === 'malformed') writeFileSync(publication, '{');
    const result = invoke(binary, ['resolver', 'channels', '--json'], {
      ...process.env, STADO_RESOLVER_STATE_FILE: join(directory, 'resolver-state.json'),
    });
    refused(result, publication);
    if (kind !== 'malformed') {
      let observed;
      try { readFileSync(publication); } catch (error) { observed = error; }
      assert.ok(observed?.errno, 'the independent filesystem operation must also refuse');
      assert.ok(`${result.stdout}\n${result.stderr}`.includes(`os error ${Math.abs(observed.errno)}`),
        'the CLI must retain the actual filesystem cause');
    } else {
      assert.ok(`${result.stdout}\n${result.stderr}`.includes('decode resolver channel publication'),
        'invalid published JSON must be attributed to decoding rather than absence');
    }
  }
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  writeFileSync(join(area, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(area, 'report.json')}`);
}
