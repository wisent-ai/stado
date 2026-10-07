import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of the GPU and cloud compute vendors as Stado compute
// providers, through the installed or selected Stado binary:
// 1. `stado config validate` on an isolated STADO_CONFIG naming each vendor
//    in `providers` without its settings refuses; it never calls the vendor
//    unknown, and for a vendor with a required setting it names the first
//    one gpu_cloud_vendors.json lists (`<vendor> provider needs <path>`);
// 2. the same configuration exposing the `cloud-<vendor>` role to jobs
//    through agent.skarbiec.secret_fields refuses, naming the role;
// 3. when STADO_GPU_TEST_PROVIDER names a vendor whose Skarbiec item is
//    tagged stado:role:cloud-<vendor> and whose settings are in the
//    operator's configuration, `stado instances list --json` reads that
//    vendor's machines from its real API (read-only: nothing is rented or
//    released).
// STADO_BIN selects the Stado under test. The report keeps every command,
// exit status and output under .build/; a failure is rethrown after the
// report is written, so the process exits unsuccessfully.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'providers-gpu-cloud-'));
const stado = process.env.STADO_BIN || 'stado';
const live = process.env.STADO_GPU_TEST_PROVIDER;
const vendors = JSON.parse(readFileSync(join(root, 'tests/providers/gpu_cloud_vendors.json'), 'utf8'));
const report = { started_at: new Date().toISOString(), vendors, live, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
// A clean exit: no spawn error, no signal, and a status that is zero (falsy).
const exitedCleanly = answer => !answer.error && !answer.signal && !answer.status;
function run(program, args, env = process.env) {
  const answer = spawnSync(program, args, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, config: env.STADO_CONFIG, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(exitedCleanly(answer), answer.stderr || answer.error?.message || answer.signal);
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
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };

  for (const [vendor, setting] of Object.entries(vendors)) {
    const unset = run(binary, ['config', 'validate'], isolated(`${vendor}-unset`, { providers: [vendor] }));
    assert.ok(!`${unset.stderr}${unset.stdout}`.includes('unknown provider'), `${vendor} is unknown: ${unset.stdout}`);
    if (setting) refused(unset, `${vendor} provider needs ${setting}`);
    const role = `cloud-${vendor}`;
    const granted = { providers: [vendor], agent: { skarbiec: { roles: [role], secret_fields: [`${role}#token`] } } };
    refused(run(binary, ['config', 'validate'], isolated(`${vendor}-granted`, granted)), role);
  }

  if (live) {
    assert.ok(live in vendors, `${live} is not one of ${Object.keys(vendors).join(', ')}`);
    const listed = JSON.parse(success(run(binary, ['instances', 'list', '--provider', live, '--json'])));
    const machines = listed.instances ?? listed;
    assert.ok(Array.isArray(machines), `${live} listing is not a list of machines`);
    report.live_instances = machines.length;
  }
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  throw error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'Configuration refusals of every GPU and cloud vendor; a read-only machine listing of one live vendor';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
