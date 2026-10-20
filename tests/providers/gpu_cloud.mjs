import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly } from '../native/process.mjs';
import { snapshotSubject, verifyRevision } from '../native/subject.mjs';

// Real CLI admission refusals for every vendor and a live read for the
// explicitly selected vendor. No live selection is a blocked qualification,
// never a pass for the integration.
// 1. `stado config validate` on an isolated STADO_CONFIG naming each vendor
//    in `providers` without its settings refuses; it never calls the vendor
//    unknown, and for a vendor with a required setting it names the first
//    one gpu_cloud_vendors.json lists (`<vendor> provider needs <path>`);
// 2. the same configuration exposing the `cloud-<vendor>` role to jobs
//    through agent.skarbiec.secret_fields refuses, naming the role;
// 3. STADO_GPU_TEST_PROVIDER is required and names a vendor whose Skarbiec
//    item is tagged stado:role:cloud-<vendor> and whose settings are in the
//    operator's configuration. `stado resources show --provider <vendor> --json`
//    reads that vendor's machines from its real API without collecting expired
//    capacity records. Nothing is rented or released; run once per vendor.
// STADO_BIN selects the Stado under test. The report keeps every command,
// exit status and output under .build/; a failure is rethrown after the
// report is written, so the process exits unsuccessfully.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'providers-gpu-cloud-'));
const stado = process.env.STADO_BIN;
const live = process.env.STADO_GPU_TEST_PROVIDER;
const vendors = JSON.parse(readFileSync(join(root, 'tests/providers/gpu_cloud_vendors.json'), 'utf8'));
const report = { started_at: new Date().toISOString(), vendors, live, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args, env = process.env) {
  const answer = spawnSync(program, args, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, config: env.STADO_CONFIG, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(exitedCleanly(answer), JSON.stringify({ stderr: answer.stderr, error: answer.error?.message,
    signal: answer.signal, status: answer.status }));
  return answer.stdout.trim();
}
function refused(answer, word) {
  assert.ok(!exitedCleanly(answer), `accepted: ${answer.stdout}`);
  assert.ok(`${answer.stderr}${answer.stdout}`.includes(word), `the refusal does not name ${word}: ${answer.stderr}`);
}
function isolated(name, document) {
  const path = join(output, `${name}.json`);
  writeFileSync(path, `${JSON.stringify(document)}\n`);
  return { ...process.env, STADO_CONFIG: path };
}
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  if (!stado || !live) {
    report.result = 'blocked';
    throw new Error(`Live vendor qualification requires STADO_BIN and STADO_GPU_TEST_PROVIDER; missing: ${[
      !stado && 'STADO_BIN', !live && 'STADO_GPU_TEST_PROVIDER',
    ].filter(Boolean).join(', ')}`);
  }
  assert.equal(success(run('git', ['status', '--porcelain', '--untracked-files=all'])), '',
    'vendor qualification requires a clean committed source tree, including no untracked files');
  assert.ok(live in vendors, `${live} is not one of ${Object.keys(vendors).join(', ')}`);
  report.stado = snapshotSubject(stado, output);
  const binary = report.stado.path;
  report.stado.version = success(run(binary, ['--version']));
  verifyRevision(report.stado.version, report.source_revision);

  for (const [vendor, setting] of Object.entries(vendors)) {
    const unset = run(binary, ['config', 'validate'], isolated(`${vendor}-unset`, { providers: [vendor] }));
    assert.ok(!`${unset.stderr}${unset.stdout}`.includes('unknown provider'), `${vendor} is unknown: ${unset.stdout}`);
    if (setting) refused(unset, `${vendor} provider needs ${setting}`);
    const role = `cloud-${vendor}`;
    const granted = { providers: [vendor], agent: { skarbiec: { roles: [role], secret_fields: [`${role}#token`] } } };
    refused(run(binary, ['config', 'validate'], isolated(`${vendor}-granted`, granted)), role);
  }

  const inventory = JSON.parse(success(run(binary, ['resources', 'show', '--provider', live, '--json'])));
  assert.equal(inventory.read_only, true, 'resource inventory did not declare a read-only observation');
  const provider = inventory.compute?.data?.providers?.find(row => row.provider === live);
  assert.ok(provider, `${live} has no compute inventory row`);
  assert.ok(Number.isSafeInteger(provider.instance_count) && provider.instance_count >= 0,
    `${live} provider enumeration did not succeed: ${JSON.stringify(provider)}`);
  assert.ok(Array.isArray(provider.instances), `${live} listing has no instances array`);
  assert.equal(provider.instances.length, provider.instance_count, `${live} instance count disagrees with its rows`);
  report.live_instances = provider.instance_count;
  report.live_coverage = provider.state;
  report.result = 'passed';
} catch (error) {
  report.error = String(error);
  throw error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'Configuration refusals of every GPU and cloud vendor; a read-only resource inventory of one live vendor';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
