import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of the refusal `stado host run attach` gives a program
// outside the managed run area. The Stado under test is handed the target
// account's installed `$HOME/.stado/bin/stado`, then a path with a `..`
// component that climbs out of a run. Each must be refused with a usage error
// before any host is asked, and the refusal must name what does the job
// instead: `stado host run deliver`, `stado host upgrade TARGET` and
// `stado host exec TARGET`. Before the repair the refusal named only the path
// rule, so a caller who wanted the installed Stado to install an archive
// retried the same call. The `--json` form must carry the same words in its
// error. Nothing is delivered and no run tree is created. Required: STADO_BIN,
// STADO_ATTACH_TEST_HOST and STADO_ATTACH_TEST_HOME (the home of the account
// Stado reaches on that host).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const required = name => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
};
const stado = required('STADO_BIN');
const host = required('STADO_ATTACH_TEST_HOST');
const remoteHome = required('STADO_ATTACH_TEST_HOME');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'host-run-attach-refused-'));
const report = { started_at: new Date().toISOString(), host, commands: [], result: 'failed' };
report.stado_sha256 = createHash('sha256').update(readFileSync(stado)).digest('hex');

function run(args) {
  const started = Date.now();
  const answer = spawnSync(stado, args, { encoding: 'utf8' });
  const record = { args, status: answer.status, signal: answer.signal, seconds: (Date.now() - started) / 1000,
    stdout: answer.stdout, stderr: answer.stderr };
  report.commands.push(record);
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, 2));
  return record;
}

function assertNamesTheWay(text, path) {
  assert.ok(text.includes(`'${path}'`), `the refusal names the path it was handed: ${text}`);
  assert.match(text, /must be an absolute path below the target account's \$HOME\/\.stado\/work\/runs/);
  assert.match(text, /attach starts only a program delivered into a run of its own/);
  assert.ok(text.includes(`stado host run deliver ${host} <file> .stado/work/runs/<id>/<name>`),
    `the refusal names the delivery that makes a program attachable: ${text}`);
  assert.ok(text.includes(`stado host upgrade ${host}`), `the refusal names the upgrade path: ${text}`);
  assert.ok(text.includes(`stado host exec ${host}`), `the refusal names the approved reads: ${text}`);
}

const usage = run(['host', 'run', 'attach', '--help']);
assert.equal(usage.status, 0, usage.stderr);
// Clap's usage status is the one a refused argument returns.
const misuse = run(['host', 'run', 'attach']);
assert.notEqual(misuse.status, 0, 'attach without TARGET and --program is refused');
const usageStatus = misuse.status;

try {
  for (const program of [`${remoteHome}/.stado/bin/stado`, `${remoteHome}/.stado/work/runs/x/../../../bin/stado`]) {
    const plain = run(['host', 'run', 'attach', host, '--program', program, '--arg', 'release']);
    assert.equal(plain.status, usageStatus, `a program outside the run area is a usage error: ${plain.stderr}`);
    assertNamesTheWay(plain.stderr, program);

    const json = run(['host', 'run', 'attach', host, '--program', program, '--json']);
    assert.equal(json.status, usageStatus, `the --json refusal is the same usage error: ${json.stderr}`);
    const document = JSON.parse(json.stdout);
    assert.equal(document.status, 'error', json.stdout);
    assert.equal(document.error_code, 'refused', json.stdout);
    assertNamesTheWay(`${document.message}\n${document.help}`, program);
  }
  report.result = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, 2));
  console.log(`report: ${join(output, 'report.json')} (${report.result})`);
}
