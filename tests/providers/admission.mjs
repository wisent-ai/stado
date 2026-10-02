import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = process.env.STADO_BIN || 'stado';
const runs = join(root, 'build/real-tests/provider-admission');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const home = join(output, 'home');
mkdirSync(home, { mode: 0o700 });
const config = join(home, 'stado.config.json');
const environment = { PATH: process.env.PATH, HOME: home, STADO_CONFIG: config };
const report = {
  started_at: new Date().toISOString(), binary, commands: [], cases: [], verdict: 'failed',
  scope: 'Real CLI provider-admission refusals and unchanged isolated configuration; no cloud enumeration, cancellation or graphical qualification.',
};

function run(program, args, isolated = true) {
  const result = spawnSync(program, args, {
    cwd: root, env: isolated ? environment : process.env,
    encoding: 'utf8', maxBuffer: 8 * 1024 * 1024,
  });
  report.commands.push({
    program, args, isolated, exit_status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message,
  });
  return result;
}

function success(result) {
  assert.equal(result.status, 0, String(result.stderr || result.error || result.signal));
  return result.stdout.trim();
}

function refusal(provider, fenced) {
  const document = {
    providers: fenced ? [provider] : [],
    providers_disabled: fenced ? [provider] : [],
  };
  const before = `${JSON.stringify(document, null, 2)}\n`;
  writeFileSync(config, before, { mode: 0o600 });
  const result = run(binary, ['instances', 'list', '--provider', provider, '--json']);
  assert.equal(result.status, 1, `${result.stdout}\n${result.stderr}`);
  const response = JSON.parse(result.stdout);
  assert.equal(response.status, 'error');
  assert.equal(response.error_code, 'refused');
  assert.equal(response.retryable, false);
  const expected = fenced ? 'PROVIDER_DISABLED' : 'PROVIDER_NOT_ENABLED';
  assert.equal(response.message.split(':', 1)[0], expected);
  assert.equal(readFileSync(config, 'utf8'), before, 'inspection changed its provider policy');
  report.cases.push({ provider, input: document, expected, response, configuration_unchanged: true });
}

try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD'], false));
  report.test_sha256 = createHash('sha256').update(readFileSync(fileURLToPath(import.meta.url))).digest('hex');
  report.binary_version = success(run(binary, ['--version'], false));
  if (!report.binary_version.includes(report.source_revision)) {
    report.verdict = 'blocked';
    throw Object.assign(new Error(`Installed Stado does not identify source ${report.source_revision}: ${report.binary_version}`), {
      code: 'STADO_REVISION_NOT_INSTALLED',
    });
  }
  for (const provider of ['azure', 'aws', 'gcp']) {
    refusal(provider, true);
    refusal(provider, false);
  }
  report.verdict = 'passed';
} catch (error) {
  report.error = { code: error.code, message: error.message, stack: error.stack };
  process.exitCode = 1;
} finally {
  try {
    rmSync(home, { recursive: true, force: true });
  } catch (error) {
    report.cleanup_error = error.message;
    report.verdict = 'failed';
    process.exitCode = 1;
  }
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
