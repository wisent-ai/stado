import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real, read-only journey of `stado database cost --provider supabase`
// against the Supabase organizations the operator's management token sees.
// Every project's add-ons and status in the report are held against the
// management API read independently with the same token, and its hourly sum
// against the add-on prices; the refusal for a provider without a bill is
// checked. Nothing is written to Supabase or to the vault. Required:
// STADO_BIN (the build under test) and SUPABASE_TEST_TOKEN_FILE (a file whose
// whole content is a management API token, read by this test only to witness
// the report). No production defaults.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const build = realpathSync(join(root, '.build'));
function required(variable) {
  assert.ok(process.env[variable], `${variable} must be stated for this journey`);
  return process.env[variable];
}
const stado = resolve(required('STADO_BIN'));
const token = readFileSync(resolve(required('SUPABASE_TEST_TOKEN_FILE')), 'utf8').trim();
const output = mkdtempSync(join(build, 'database-cost-'));
const report = { started_at: new Date().toISOString(), commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(args) {
  const answer = spawnSync(stado, args, { cwd: root, env: process.env, encoding: 'utf8' });
  report.commands.push({ program: stado, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
async function api(path) {
  const response = await fetch(`https://api.supabase.com/v1${path}`, {
    headers: { Authorization: `Bearer ${token}`, 'User-Agent': 'stado-database-cost-test' },
  });
  assert.ok(response.ok, `Supabase GET ${path} answered ${response.status}`);
  return response.json();
}
let failure;
try {
  report.source_revision = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim();
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.stado = { path: stado, sha256: digest(readFileSync(stado)) };

  const fleet = run(['database', 'cost', '--provider', 'fleet', '--json']);
  assert.notEqual(fleet.status, constants.F_OK);
  assert.ok(fleet.stderr.includes('has no bill to read'), fleet.stderr);

  const answer = run(['database', 'cost', '--provider', 'supabase', '--json']);
  assert.equal(answer.status, constants.F_OK, answer.stderr);
  const bill = JSON.parse(answer.stdout);
  const projects = await api('/projects');
  assert.deepEqual(bill.projects.map(row => row.ref).sort(), projects.map(project => project.ref).sort(),
    'The report must cover every project the token sees');
  for (const project of projects) {
    const row = bill.projects.find(entry => entry.ref === project.ref);
    assert.equal(row.status, project.status);
    const addons = (await api(`/projects/${project.ref}/billing/addons`)).selected_addons;
    assert.deepEqual(row.addons.map(line => line.variant).sort(), addons.map(addon => addon.variant.id).sort(),
      `${project.ref}: the report's add-ons must be the project's`);
    const running = project.status === 'ACTIVE_HEALTHY';
    let hourly = Number();
    for (const addon of addons) {
      if (addon.variant.price.interval === 'hourly' && (running || addon.type !== 'compute_instance')) {
        hourly += addon.variant.price.amount;
      }
    }
    assert.ok(Math.abs(row.hourly_usd - hourly) < Number.EPSILON * addons.length, `${project.ref}: hourly sum`);
  }
  for (const organization of bill.organizations) {
    const plan = (await api(`/organizations/${organization.organization}`)).plan;
    assert.equal(organization.plan, plan);
  }
  report.bill = bill;
  report.result = 'passed';
} catch (error) {
  failure = error;
  report.error = error.stack;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'Real read-only Supabase bill against the management API; no project, item or vault changed';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
