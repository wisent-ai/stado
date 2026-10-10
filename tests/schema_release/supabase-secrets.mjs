import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants as permissions, writeFileSync } from 'node:fs';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const root = fileURLToPath(new URL('../../', import.meta.url));
const privateFile = permissions.S_IRUSR | permissions.S_IWUSR;

test('verified schema release applies declared function secrets to their real consumer', async () => {
  const directory = path.join(root, '.build', 'supabase-secrets');
  await mkdir(directory, { recursive: true, mode: permissions.S_IRWXU });
  const work = await mkdtemp(path.join(directory, 'run-'));
  const output = path.join(work, 'output');
  await mkdir(output);
  const report = { outcome: 'running', commands: [], requests: [] };
  const reportPath = path.join(work, 'report.json');
  process.once('exit', exitCode => {
    report.exitCode = exitCode;
    writeFileSync(reportPath, JSON.stringify(report), { mode: privateFile });
  });
  async function command(program, args, env) {
    const record = { program, args, stdout: '', stderr: '' };
    report.commands.push(record);
    try {
      const child = spawn(program, args, { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
      child.stdout.on('data', bytes => { record.stdout += bytes.toString(); });
      child.stderr.on('data', bytes => { record.stderr += bytes.toString(); });
      await new Promise((resolve, reject) => {
        child.once('error', reject);
        child.once('close', (code, signal) => { record.exitCode = code; record.signal = signal; resolve(); });
      });
      return record;
    } catch (error) {
      record.error = error.message;
      throw error;
    } finally {
      await writeFile(reportPath, JSON.stringify(report), { mode: privateFile });
    }
  }
  let restore;
  try {
    const file = process.env.STADO_SUPABASE_SECRETS_FIXTURE;
    const binary = process.env.STADO_TEST_BINARY;
    assert.ok(file && binary, 'Declare STADO_SUPABASE_SECRETS_FIXTURE and the real STADO_TEST_BINARY');
    const fixtureBytes = await readFile(file);
    const input = JSON.parse(fixtureBytes);
    report.fixtureSha256 = createHash('sha256').update(fixtureBytes).digest('hex');
    assert.equal(input.isolated, true, 'Only a dedicated existing Supabase project is permitted');
    assert.match(input.projectRef, /^[a-z]+$/);
    assert.ok(input.product && input.version && input.releaseUri && input.releaseArchive && input.releaseSha256);
    assert.ok(Array.isArray(input.secretNames) && input.secretNames.some(name => typeof name === 'string'));
    assert.equal(new Set(input.secretNames).size, input.secretNames.length);
    const revision = await command('git', ['rev-parse', 'HEAD'], process.env);
    assert.equal(revision.signal, null);
    assert.equal(revision.stderr, '');
    report.sourceRevision = revision.stdout.trim();
    assert.equal(input.sourceRevision, report.sourceRevision);
    const version = await command(binary, ['--version'], process.env);
    assert.equal(version.signal, null);
    assert.ok(version.stdout.split(/[\s()]+/).includes(report.sourceRevision), 'Candidate binary must identify this source revision');
    report.binarySha256 = createHash('sha256').update(await readFile(binary)).digest('hex');
    const changed = await command('git', ['status', '--porcelain', '--untracked-files=normal', '--',
      'stado-rs/product/src/release_steps/supabase', 'tests/schema_release'], process.env);
    assert.equal(changed.stdout, '', 'Commit the qualified implementation and journey first');
    const archive = path.resolve(path.dirname(path.resolve(file)), input.releaseArchive);
    assert.equal(createHash('sha256').update(await readFile(archive)).digest('hex'), input.releaseSha256);
    const token = process.env.SUPABASE_ACCESS_TOKEN;
    const probeKey = process.env[input.probeKeyEnv];
    assert.ok(token && probeKey, 'Management token and isolated probe credentials must be injected');
    const base = new URL(`https://${input.projectRef}.supabase.co`);
    const probe = new URL(input.probePath, base);
    assert.equal(probe.origin, base.origin);
    assert.ok(probe.pathname.startsWith('/functions/v1/'));
    assert.notDeepEqual(input.expectedBefore, input.expectedInitial);
    assert.notDeepEqual(input.expectedInitial, input.expectedRotated);
    async function observe(expected) {
      const record = { url: probe.href, method: 'GET' };
      report.requests.push(record);
      const response = await fetch(probe, { redirect: 'error', credentials: 'omit', cache: 'no-store',
        headers: { apikey: probeKey, Authorization: `Bearer ${probeKey}` } });
      record.status = response.status;
      record.body = await response.text();
      await writeFile(reportPath, JSON.stringify(report), { mode: privateFile });
      assert.ok(response.ok, `${probe.href}: HTTP ${response.status}: ${record.body}`);
      assert.deepEqual(JSON.parse(record.body), expected);
    }
    async function migrationHistory() {
      const endpoint = new URL('https://api.supabase.com/v1/projects/');
      endpoint.pathname += `${encodeURIComponent(input.projectRef)}/database/migrations`;
      const record = { url: endpoint.href, method: 'GET' };
      report.requests.push(record);
      const response = await fetch(endpoint, { redirect: 'error', credentials: 'omit', cache: 'no-store',
        headers: { Authorization: `Bearer ${token}` } });
      record.status = response.status;
      record.body = await response.text();
      await writeFile(reportPath, JSON.stringify(report), { mode: privateFile });
      assert.ok(response.ok, `${endpoint.href}: HTTP ${response.status}: ${record.body}`);
      const rows = JSON.parse(record.body);
      assert.ok(Array.isArray(rows), 'Migration history must be a JSON array');
      return rows.map(row => {
        assert.equal(typeof row.version, 'string');
        assert.ok(row.version.trim(), 'Migration history contains a blank version');
        return row.version;
      }).sort();
    }
    assert.ok(Array.isArray(input.expectedMigrationVersions), 'Declare the delivered migration versions');
    const historyBefore = await migrationHistory();
    report.historyBefore = historyBefore;
    const environment = { ...process.env, WISENT_OUTPUT_DIR: output,
      WISENT_RELEASE_ARCHIVE: archive, WISENT_RELEASE_SHA256: input.releaseSha256,
      WISENT_RELEASE_URI: input.releaseUri, WISENT_PRODUCT: input.product,
      WISENT_VERSION: input.version, SUPABASE_PROJECT_REF: input.projectRef };
    for (const name of input.secretNames) delete environment[name];
    const args = ['product', 'deliver', 'schema', '--engine', 'supabase'];
    await observe(input.expectedBefore);
    const missing = await command(binary, args, environment);
    assert.equal(missing.signal, null);
    assert.notEqual(missing.exitCode, revision.exitCode, 'Missing credentials must fail the delivery');
    assert.ok(missing.stderr.includes('resolving declared function secret'), missing.stderr);
    assert.ok(input.secretNames.some(name => missing.stderr.includes(name)), missing.stderr);
    await observe(input.expectedBefore);
    const original = { ...environment };
    for (const name of input.secretNames) {
      const value = process.env[input.originalSecretEnv[name]];
      assert.ok(value, `Missing original isolated qualification value for ${name}`);
      original[name] = value;
    }
    restore = async () => {
      const restored = await command(binary, args, original);
      assert.equal(restored.exitCode, revision.exitCode, restored.stderr);
      assert.equal(restored.signal, null);
      await observe(input.expectedBefore);
      report.originalSecretStateRestored = true;
    };
    const identified = { ...environment };
    for (const name of input.secretNames) {
      const value = process.env[input.initialSecretEnv[name]];
      assert.ok(value, `Missing injected qualification value for ${name}`);
      identified[name] = value;
    }
    report.identityRefusals = [];
    for (const name of ['WISENT_PRODUCT', 'WISENT_VERSION', 'WISENT_RELEASE_URI']) {
      for (const value of [undefined, '', ' \t']) {
        const selected = { ...identified };
        if (value === undefined) delete selected[name];
        else selected[name] = value;
        report.identityRefusals.push({ name, present: value !== undefined, value });
        const refused = await command(binary, args, selected);
        assert.equal(refused.signal, null);
        assert.notEqual(refused.exitCode, revision.exitCode, `Missing release identity ${name} must refuse`);
        assert.ok(refused.stderr.includes(name), refused.stderr);
        await observe(input.expectedBefore);
      }
    }
    report.passwordRefusals = [];
    for (const value of ['', ' \t']) {
      report.passwordRefusals.push({ input: value });
      const refused = await command(binary, args, { ...identified, SUPABASE_DB_PASSWORD: value });
      assert.equal(refused.signal, null);
      assert.notEqual(refused.exitCode, revision.exitCode, 'An explicitly blank password must not select another authentication method');
      assert.ok(refused.stderr.includes('SUPABASE_DB_PASSWORD') && refused.stderr.includes('blank'), refused.stderr);
      await observe(input.expectedBefore);
    }
    const declarationCases = [
      ['split-parts-missing', 'split-migrations.json', 'parts'],
      ['split-parts-type', 'split-migrations.json', 'map'],
      ['split-original-type', 'split-migrations.json', 'string'],
      ['split-blank-part', 'split-migrations.json', 'parts key'],
      ['split-blank-original', 'split-migrations.json', 'parts value'],
      ['baseline-applied-missing', 'baseline.json', 'applied'],
      ['baseline-retired-missing', 'baseline.json', 'retired'],
      ['baseline-applied-type', 'baseline.json', 'sequence'],
      ['baseline-retired-type', 'baseline.json', 'sequence'],
      ['baseline-version-type', 'baseline.json', 'string'],
      ['baseline-blank-version', 'baseline.json', 'blank version'],
    ];
    report.declarationRefusals = [];
    for (const [name, declaration, cause] of declarationCases) {
      const selected = input.declarationRefusals?.[name];
      assert.ok(selected?.releaseArchive && selected?.releaseSha256, `Declare the real release archive for ${name}`);
      const selectedArchive = path.resolve(path.dirname(path.resolve(file)), selected.releaseArchive);
      const digest = createHash('sha256').update(await readFile(selectedArchive)).digest('hex');
      assert.equal(digest, selected.releaseSha256);
      assert.notEqual(digest, input.releaseSha256, `${name} must carry a changed declaration`);
      const selectedOutput = path.join(work, name);
      await mkdir(selectedOutput);
      const refused = await command(binary, args, { ...identified,
        WISENT_RELEASE_ARCHIVE: selectedArchive, WISENT_RELEASE_SHA256: digest,
        WISENT_OUTPUT_DIR: selectedOutput });
      assert.equal(refused.signal, null);
      assert.notEqual(refused.exitCode, revision.exitCode, `${name} must refuse delivery`);
      assert.ok(refused.stderr.includes('invalid migration declaration')
        && refused.stderr.includes(declaration) && refused.stderr.includes(cause), refused.stderr);
      await assert.rejects(readFile(path.join(selectedOutput, 'supabase-receipt.json')), { code: 'ENOENT' });
      await observe(input.expectedBefore);
      assert.deepEqual(await migrationHistory(), historyBefore, `${name} changed migration history`);
      report.declarationRefusals.push({ name, archive: selectedArchive, sha256: digest,
        command: report.commands.length, consumerUnchanged: true });
      await writeFile(reportPath, JSON.stringify(report), { mode: privateFile });
    }
    for (const [bindings, expected] of [[input.initialSecretEnv, input.expectedInitial], [input.rotatedSecretEnv, input.expectedRotated]]) {
      for (const name of input.secretNames) {
        const value = process.env[bindings[name]];
        assert.ok(value, `Missing injected qualification value for ${name}`);
        environment[name] = value;
      }
      const historyBeforeDelivery = await migrationHistory();
      const delivered = await command(binary, args, environment);
      assert.equal(delivered.signal, null);
      const receiptBytes = await readFile(path.join(output, 'supabase-receipt.json'));
      const receipt = JSON.parse(receiptBytes);
      assert.equal(receipt.release_sha256, input.releaseSha256);
      assert.equal(receipt.project_ref, input.projectRef);
      assert.deepEqual(receipt.history_before, historyBeforeDelivery);
      assert.equal(receipt.function_secrets.state, 'accepted');
      assert.deepEqual([...receipt.function_secrets.names].sort(), [...input.secretNames].sort());
      for (const name of input.secretNames) assert.ok(!receiptBytes.includes(Buffer.from(environment[name])), `Receipt leaked ${name}`);
      await writeFile(path.join(work, `receipt-${report.commands.length}.json`), receiptBytes, { mode: privateFile });
      await observe(expected);
      assert.deepEqual(await migrationHistory(), [...input.expectedMigrationVersions].sort());
      // A receipt from an earlier attempt cannot make a failed delivery pass.
      assert.equal(delivered.exitCode, revision.exitCode, delivered.stderr);
    }
    report.outcome = 'passed';
  } catch (error) {
    report.outcome = 'failed';
    report.error = error.stack;
    throw error;
  } finally {
    if (restore) {
      try {
        await restore();
      } catch (error) {
        report.outcome = 'failed';
        report.restorationError = error.stack;
        await writeFile(reportPath, JSON.stringify(report), { mode: privateFile });
        throw error;
      }
    }
    await writeFile(reportPath, JSON.stringify(report), { mode: privateFile });
    console.log(`Supabase secret delivery evidence: ${reportPath}`);
  }
});
