import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly } from '../native/process.mjs';

// Real journey: the Azure cloud a deployment declares is the one every Azure
// call reaches. The Stado under test runs with a HOME and STADO_CONFIG of its
// own under this checkout's .build; a loopback listener stands in for Azure.
//
// 1. Declared: azure.cloud.resource_manager, azure.cloud.authority,
//    azure.cloud.identity_endpoint and azure.cloud.retail_prices name the
//    listener. `stado config show --json` names them under
//    resolved.azure_cloud, the Resource Manager's trailing slash dropped.
//    `stado quota catalog --provider azure --json`
//    takes a managed-identity token from the declared identity endpoint,
//    asked for the declared Resource Manager as its resource, and reads the
//    SKU table from the declared Resource Manager with that token; the GPU SKU
//    the listener serves is a catalog row.
// 2. Refused: an authority that is neither HTTPS nor on a loopback or
//    link-local address. config show names the refusal under
//    resolved.azure_cloud.refused, the catalog's Azure row is refused with the
//    same sentence naming the binding, and the listener is asked nothing.
// 3. Undeclared: config show names the public cloud's endpoints.
//
// Required: STADO_BIN, the Stado executable under test. Every command, exit
// status, output and request the listener received is kept in report.json.
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN is required: the Stado executable under test');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'azure-cloud-'));
const report = { started_at: new Date().toISOString(), commands: [], requests: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

const SUBSCRIPTION = randomUUID();
const TOKEN = `journey-${randomUUID()}`;
// The lifetime the identity endpoint states for its token, in seconds; IMDS
// states it as a string.
const TOKEN_LIFETIME = '3600';
const GPU_SKU = { name: 'Standard_NC4as_T4_v3', family: 'standardNCASv3_T4Family', location: 'eastus' };
const PUBLIC = {
  resource_manager: 'https://management.azure.com',
  authority: 'https://login.microsoftonline.com',
  identity_endpoint: 'http://169.254.169.254/metadata/identity/oauth2/token',
  retail_prices: 'https://prices.azure.com/api/retail/prices',
};

// The listener standing in for Azure: the identity endpoint answers a token,
// the Resource Manager answers the SKU table to a request carrying it, and
// anything else is Not Found. Every request is recorded.
function azure() {
  const server = createServer((request, response) => {
    const url = new URL(request.url, 'http://listener');
    report.requests.push({ method: request.method, path: url.pathname, query: Object.fromEntries(url.searchParams),
      metadata: request.headers.metadata ?? null, authorization: request.headers.authorization ?? null });
    const answer = (status, body) => {
      response.writeHead(status, { 'content-type': 'application/json' });
      response.end(JSON.stringify(body));
    };
    if (url.pathname === '/identity/oauth2/token' && request.headers.metadata === 'true') {
      return answer(200, { access_token: TOKEN, expires_in: TOKEN_LIFETIME });
    }
    if (url.pathname === `/arm/subscriptions/${SUBSCRIPTION}/providers/Microsoft.Compute/skus`
      && request.headers.authorization === `Bearer ${TOKEN}`) {
      return answer(200, { value: [{ name: GPU_SKU.name, family: GPU_SKU.family, locations: [GPU_SKU.location],
        capabilities: [{ name: 'GPUs', value: '1' }] }] });
    }
    return answer(404, { error: { code: 'NotFound', message: `the listener serves no ${url.pathname}` } });
  });
  return new Promise(ready => server.listen({ host: '127.0.0.1', port: 0 }, () => ready(server)));
}

// A Stado run that does not block this process, so the listener answers it.
function run(name, environment, args) {
  return new Promise(done => {
    const child = spawn(stado, args, { cwd: root, env: environment });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', chunk => { stdout += chunk; });
    child.stderr.on('data', chunk => { stderr += chunk; });
    child.on('close', (status, signal) => {
      const answer = { status, signal, stdout, stderr };
      report.commands.push({ case: name, program: stado, arguments: args, exit_status: status, signal, stdout, stderr });
      done(answer);
    });
  });
}

function deployment(name, declared) {
  const home = join(output, name);
  mkdirSync(home, { recursive: true });
  return { HOME: home, PATH: process.env.PATH, TMPDIR: home, STADO_CONFIG: join(home, 'config.json'),
    AZURE_SUBSCRIPTION_ID: SUBSCRIPTION, ...declared };
}

async function resolved(name, environment) {
  const shown = await run(name, environment, ['config', 'show', '--json']);
  assert.ok(exitedCleanly(shown), `${name}: config show failed: ${shown.stderr}`);
  return JSON.parse(shown.stdout).resolved.azure_cloud;
}

const server = await azure();
try {
  report.source_revision = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim();
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.stado_sha256 = digest(readFileSync(stado));
  const origin = `http://127.0.0.1:${server.address().port}`;

  const declared = deployment('declared', { AZURE_RESOURCE_MANAGER: `${origin}/arm/`, AZURE_AUTHORITY: `${origin}/authority`,
    AZURE_IDENTITY_ENDPOINT: `${origin}/identity/oauth2/token`, AZURE_RETAIL_PRICES: `${origin}/prices` });
  assert.ok(exitedCleanly(await run('declared', declared, ['config', 'init'])), 'config init failed');
  assert.deepEqual(await resolved('declared', declared), { resource_manager: `${origin}/arm`,
    authority: `${origin}/authority`, identity_endpoint: `${origin}/identity/oauth2/token`,
    retail_prices: `${origin}/prices` });
  const catalog = await run('declared', declared, ['quota', 'catalog', '--provider', 'azure', '--json']);
  assert.ok(exitedCleanly(catalog), `quota catalog failed: ${catalog.stderr}`);
  const rows = JSON.parse(catalog.stdout).azure;
  assert.ok(rows.some(row => row.sku === GPU_SKU.name && row.family === GPU_SKU.family
    && row.location === GPU_SKU.location && row.gpus === 1), `the served GPU SKU is not a catalog row: ${catalog.stdout}`);
  const identity = report.requests.find(request => request.path === '/identity/oauth2/token');
  assert.ok(identity, 'the declared identity endpoint was not asked for a token');
  assert.equal(identity.query.resource, `${origin}/arm`, 'the token was not asked for the declared Resource Manager');
  const skus = report.requests.find(request => request.path.endsWith('/providers/Microsoft.Compute/skus'));
  assert.equal(skus?.authorization, `Bearer ${TOKEN}`, 'the SKU table was not read with the identity token');

  const asked = report.requests.length;
  const refusedAuthority = 'http://authority.invalid';
  const refused = deployment('refused', { AZURE_RESOURCE_MANAGER: `${origin}/arm`, AZURE_AUTHORITY: refusedAuthority,
    AZURE_IDENTITY_ENDPOINT: `${origin}/identity/oauth2/token` });
  assert.ok(exitedCleanly(await run('refused', refused, ['config', 'init'])), 'config init failed');
  const sentence = `azure.cloud.authority (environment override AZURE_AUTHORITY) must be an HTTPS URL, or one on a loopback or link-local address; "${refusedAuthority}" is neither`;
  assert.deepEqual(await resolved('refused', refused), { refused: sentence });
  const refusedCatalog = await run('refused', refused, ['quota', 'catalog', '--provider', 'azure', '--json']);
  const refusedRows = JSON.parse(refusedCatalog.stdout).azure;
  assert.ok(refusedRows.some(row => row.ok === false && row.error === sentence),
    `the catalog did not refuse with the declaration's sentence: ${refusedCatalog.stdout}`);
  assert.equal(report.requests.length, asked, 'a refused declaration still sent a request');

  const undeclared = deployment('undeclared', {});
  assert.ok(exitedCleanly(await run('undeclared', undeclared, ['config', 'init'])), 'config init failed');
  assert.deepEqual(await resolved('undeclared', undeclared), PUBLIC);

  report.result = 'passed';
} finally {
  server.close();
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, 2));
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
