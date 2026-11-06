import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly, exitedRefusing } from '../native/process.mjs';

// Real journey: where `stado scheduler tick` places an Azure agent VM, decided
// by the error.code ARM states. Each case is its own deployment under this
// checkout's .build: HOME, STADO_CONFIG, a local queue store, a real Skarbiec
// (SKARBIEC_BIN) holding the billing and workload items, the real Oko
// (OKO_BIN) answering the credit floor from a rulings register of the case's
// own, and one loopback listener declared as the Azure cloud (Resource
// Manager, authority, identity endpoint, Retail Prices). One queued T4 job,
// two locations: eastus answers the NIC create QuotaExceeded in every case.
// westus then answers:
// 1. skip:     VM create SkuNotAvailable, NIC delete 404 -> exit 0, nothing
//              placed, the NIC delete asked;
// 2. uncoded:  VM create HTTP 500 stating no code -> refused naming
//              "no Azure location placed" and westus' VM refusal, not eastus;
// 3. nic-409:  VM create SkuNotAvailable, NIC delete 409 NicInUse -> refused
//              naming the VM, westus and both refusals;
// 4. lro:      VM create accepted, its operation ends Failed with
//              SkuNotAvailable -> read as that code: exit 0, nothing placed.
// 5. beside:   while the lro case's tick waits on its operation (the poll is
//              answered only after this), a second `stado scheduler tick` on
//              the same store is refused naming the first tick's pid and the
//              lock record; once the first ends, the store's lock is gone.
// Required: STADO_BIN, SKARBIEC_BIN, OKO_BIN. report.json keeps every command,
// status, output and request.
const required = name => { assert.ok(process.env[name], `${name} is required`); return realpathSync(process.env[name]); };
const [stado, skarbiec, oko] = ['STADO_BIN', 'SKARBIEC_BIN', 'OKO_BIN'].map(required);
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'azure-placement-'));
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const report = { started_at: new Date().toISOString(), commands: [], cases: {}, result: 'failed',
  stado_sha256: digest(readFileSync(stado)), test_sha256: digest(readFileSync(fileURLToPath(import.meta.url))),
  source_revision: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim() };

const SUBSCRIPTION = randomUUID();
const TENANT = randomUUID();
const GROUP = 'journey-group';
const [NO_ROOM, WESTUS] = ['eastus', 'westus'];
const SIZE = 'Standard_NC4as_T4_v3';
// Stated by the listener as Azure states them: the T4 family's vCPU limit
// (two four-vCPU slots), the credit balance, the hourly price, and a floor
// below balance less one hour, so the gate admits.
const [FAMILY_VCPUS, BALANCE, HOURLY, FLOOR] = [8, 1000, 1, 10];
const FLOOR_SUBJECT = `capability:cloud@credits-floor:azure:${FLOOR}:USD`;
const refusal = (code, message) => ({ error: { ...(code && { code }), message } });

function listener(name, westus) {
  const requests = report.cases[name].requests;
  return createServer((request, response) => {
    const url = new URL(request.url, 'http://listener');
    requests.push({ method: request.method, path: url.pathname });
    const answer = (status, body, headers = {}) => {
      response.writeHead(status, { 'content-type': 'application/json', ...headers });
      response.end(JSON.stringify(body));
    };
    const arm = `/arm/subscriptions/${SUBSCRIPTION}`;
    const nic = url.pathname.match(/\/networkInterfaces\/([^/]+)$/);
    const vm = url.pathname.match(/\/virtualMachines\/([^/]+)$/);
    if (url.pathname === '/identity') return answer(200, { access_token: 'identity', expires_in: '3600' });
    if (url.pathname === `/authority/${TENANT}/oauth2/v2.0/token`) return answer(200, { access_token: 'billing', expires_in: 3600 });
    if (url.pathname === '/prices') return answer(200, { Items: [NO_ROOM, WESTUS].map(region => ({ retailPrice: HOURLY,
      productName: 'Virtual Machines NCasT4_v3 Series', type: 'Consumption', meterName: 'NC4as T4 v3', meterId: region,
      unitOfMeasure: '1 Hour', currencyCode: 'USD', armRegionName: region, armSkuName: SIZE, skuName: 'NC4as T4 v3' })) });
    if (url.pathname === `${arm}/providers/Microsoft.Consumption/balances`) return answer(200, { properties: { amount: { value: BALANCE, currency: 'USD' } } });
    if (url.pathname.endsWith('/usages')) return answer(200, { value: [{ name: { value: 'standardNCASv3Family' }, limit: FAMILY_VCPUS }] });
    if (url.pathname.endsWith('/Microsoft.Compute/virtualMachines')) return answer(200, { value: [] });
    if (url.pathname === '/operations/vm') return westus.operation(() => answer(200, { status: 'Failed',
      ...refusal('SkuNotAvailable', `${SIZE} is not offered in ${WESTUS}`) }));
    if (nic && request.method === 'PUT') {
      return new Promise(read => request.on('data', () => {}).on('end', read)).then(() => {
        // The NIC's location is in its body; eastus has no room in every case.
        const location = requests.filter(r => r.path === url.pathname && r.method === 'PUT').length === 1 ? NO_ROOM : WESTUS;
        requests.at(-1).location = location;
        if (location === NO_ROOM) return answer(409, refusal('QuotaExceeded', `no ${SIZE} quota left in ${NO_ROOM}`));
        return answer(200, { id: `${arm}/resourceGroups/${GROUP}/providers/Microsoft.Network/networkInterfaces/${nic[1]}` });
      });
    }
    if (nic && request.method === 'DELETE') return westus.nicDelete(answer);
    if (vm && request.method === 'PUT') return westus.vmCreate(answer, `http://${request.headers.host}`);
    if (vm && request.method === 'GET') return answer(404, refusal('ResourceNotFound', `${vm[1]} does not exist`));
    return answer(404, refusal('NotFound', `the listener serves no ${request.method} ${url.pathname}`));
  });
}

function run(name, environment, program, args, input) {
  return new Promise(done => {
    const child = spawn(program, args, { cwd: environment.HOME, env: environment });
    let [stdout, stderr] = ['', ''];
    child.stdout.on('data', chunk => { stdout += chunk; });
    child.stderr.on('data', chunk => { stderr += chunk; });
    child.stdin.end(input ?? '');
    child.on('close', (status, signal) => {
      report.commands.push({ case: name, program, args, pid: child.pid, exit_status: status, signal, stdout, stderr });
      done({ status, signal, stdout, stderr });
    });
  });
}

async function must(name, environment, program, args, input) {
  const answer = await run(name, environment, program, args, input);
  assert.ok(exitedCleanly(answer), `${name}: ${program} ${args.join(' ')}: ${answer.stderr}`);
  return answer.stdout;
}

// A real Skarbiec of the case's own, serving on loopback until the case ends.
// gpg-agent's socket lives in the keyring and a Unix socket path is at most
// 104 bytes on macOS, so the keyring sits at a short path of its own.
async function vault(name, home, environment) {
  const keyring = join(root, '.build', 'g', randomUUID().split('-')[0]);
  mkdirSync(keyring, { recursive: true, mode: 0o700 });
  const env = { ...environment, GNUPGHOME: keyring, SKARBIEC_VAULT_FILE: join(home, 'vault.json'), SKARBIEC_AUDIT_FILE: join(home, 'audit.jsonl') };
  await must(name, env, skarbiec, ['init', 'journey-owner']);
  // Skarbiec's canonical item payload (schema skarbiec.item.v2).
  const item = fields => JSON.stringify({ schema: 'skarbiec.item.v2', kind: 'stado-secret', fields, context: {} });
  await must(name, env, skarbiec, ['set', 'azure-billing', '--tags', 'stado:role:azure-billing'],
    item({ tenant_id: TENANT, client_id: 'billing', client_secret: randomUUID(), subscription_id: SUBSCRIPTION }));
  await must(name, env, skarbiec, ['set', 'journey-workload', '--tags', 'stado:role:journey-workload'],
    item({ token: randomUUID() }));
  const grant = async (consumer, capabilities, file) => {
    const issued = JSON.parse(await must(name, env, skarbiec, ['grant', 'issue', consumer, '--capabilities', capabilities.join(',')]));
    writeFileSync(file, issued.token, { mode: 0o600 });
  };
  await grant('stado', ['tenant_id', 'client_id', 'client_secret', 'subscription_id', 'billing_account', 'billing_profile',
    'billing_profile_system_id'].map(field => `read:role:azure-billing#${field}`), join(home, 'stado.token'));
  await grant('journey-agent', ['read:role:journey-workload#token'], join(home, 'agent.token'));
  const server = spawn(skarbiec, ['serve', '--port', '0'], { env, stdio: ['ignore', 'ignore', 'pipe'] });
  const address = await new Promise((ready, failed) => {
    let said = '';
    server.stderr.on('data', chunk => { said += chunk; const found = said.match(/listening on (http:\/\/\S+)/); if (found) ready(found[1]); });
    server.on('close', status => failed(new Error(`skarbiec serve ended (${status}): ${said}`)));
  });
  return { server, address, keyring };
}

async function placement(name, westus) {
  report.cases[name] = { requests: [] };
  const home = join(output, name);
  mkdirSync(join(home, '.stado', 'bin'), { recursive: true });
  symlinkSync(oko, join(home, '.stado', 'bin', 'oko'));
  const azure = listener(name, westus);
  await new Promise(ready => azure.listen({ host: '127.0.0.1', port: 0 }, ready));
  const origin = `http://127.0.0.1:${azure.address().port}`;
  const base = { HOME: home, PATH: process.env.PATH, TMPDIR: home };
  const { server, address, keyring } = await vault(name, home, base);
  const rulings = join(home, 'rulings.json');
  writeFileSync(rulings, JSON.stringify({ rulings: [{ id: randomUUID(), subjects: [FLOOR_SUBJECT], session: randomUUID(), ordinal: 1,
    said: `the floor is ${FLOOR} USD`, recordedIn: randomUUID(), recordedAt: new Date().toISOString() }] }));
  const sshKey = `ssh-ed25519 ${Buffer.from(randomUUID()).toString('base64')} journey`;
  const environment = { ...base, STADO_CONFIG: join(home, 'config.json'), OKO_RULING_REGISTER: rulings,
    WC_PROVIDERS: 'azure', WC_STORAGE_BACKEND: 'local', WC_LOCAL_STORAGE_PATH: join(home, 'store'),
    WC_SKARBIEC_URL: address, WC_SKARBIEC_CONSUMER: 'stado', WC_SKARBIEC_TOKEN_FILE: join(home, 'stado.token'),
    // The agent's broker is named as rented machines reach it; the credential
    // store routes every read here to the case's own Skarbiec.
    WC_AGENT_SKARBIEC_URL: 'https://vault.agent.invalid', WC_AGENT_SKARBIEC_CONSUMER: 'journey-agent',
    WC_AGENT_SKARBIEC_TOKEN_FILE: join(home, 'agent.token'), WC_AGENT_SKARBIEC_ROLES: 'journey-workload',
    AZURE_SUBSCRIPTION_ID: SUBSCRIPTION, AZURE_RESOURCE_GROUP: GROUP, AZURE_LOCATIONS: `${NO_ROOM},${WESTUS}`,
    AZURE_VNET: 'journey-vnet', AZURE_SUBNET: 'journey-subnet', AZURE_NSG: 'journey-nsg', AZURE_VM_USERNAME: 'journey',
    AZURE_IMAGE_URN: 'Canonical:ubuntu-24_04-lts:server:latest', AZURE_SSH_PUBLIC_KEY: sshKey,
    AZURE_VM_IDENTITY_ID: `/subscriptions/${SUBSCRIPTION}/resourceGroups/${GROUP}/providers/Microsoft.ManagedIdentity/userAssignedIdentities/agent`,
    AZURE_RESOURCE_MANAGER: `${origin}/arm`, AZURE_AUTHORITY: `${origin}/authority`,
    AZURE_IDENTITY_ENDPOINT: `${origin}/identity`, AZURE_RETAIL_PRICES: `${origin}/prices`,
    STADO_API_URL: 'https://api.stado.invalid', STADO_RELEASE_VERSION: '1.0.0', STADO_RELEASE_PLATFORM: 'linux-x86_64',
    STADO_AGENT_RUNTIME_BUNDLE_URI: 'stado://releases/stado/1.0.0/linux-x86_64/agent.tar.gz',
    STADO_AGENT_RUNTIME_BUNDLE_SHA256: digest(Buffer.from(name)), STADO_AGENT_POLL_SECONDS: '60' };
  try {
    await must(name, environment, stado, ['config', 'init']);
    await must(name, environment, stado, ['config', 'set', 'credentials.store', `skarbiec://${address}`]);
    await must(name, environment, stado, ['submit', 'echo placed', '--provider', 'azure', '--pin-provider',
      '--gpu-type', 'nvidia-tesla-t4', '--boot-disk-gb', '64', '--runtime-seconds-estimate', '60']);
    const first = run(name, environment, stado, ['scheduler', 'tick', '--provider', 'azure', '--json']);
    // While the first tick waits on its operation, a second one on the same
    // store must be refused naming it.
    const beside = westus.beside && await westus.beside.then(() =>
      run(`${name}-beside`, environment, stado, ['scheduler', 'tick', '--provider', 'azure', '--json']));
    westus.release?.();
    const tick = await first;
    const requests = report.cases[name].requests;
    const nicPuts = requests.filter(r => r.method === 'PUT' && r.path.includes('/networkInterfaces/'));
    assert.deepEqual(nicPuts.map(r => r.location), [NO_ROOM, WESTUS], `${name}: both locations were not tried in order`);
    const vmName = requests.find(r => r.method === 'PUT' && r.path.includes('/virtualMachines/'))?.path.split('/').at(-1);
    assert.ok(vmName, `${name}: no VM create reached ${WESTUS}`);
    const lockLeft = existsSync(join(home, 'store', 'state', 'coordinator', 'tick-lock.json'));
    return { tick, requests, vmName, beside, lockLeft };
  } finally {
    server.kill();
    azure.close();
    spawnSync('gpgconf', ['--homedir', keyring, '--kill', 'all']);
    rmSync(keyring, { recursive: true, force: true });
  }
}

const deleted = (status, body) => ({ nicDelete: answer => answer(status, body), operation: answered => answered() });
try {
  const skip = await placement('skip', { ...deleted(404, refusal('NotFound', 'gone')),
    vmCreate: answer => answer(409, refusal('SkuNotAvailable', `${SIZE} is not offered in ${WESTUS}`)) });
  assert.ok(exitedCleanly(skip.tick), `skip: the tick failed: ${skip.tick.stderr}`);
  assert.equal(JSON.parse(skip.tick.stdout).scheduled, 0);
  assert.ok(skip.requests.some(r => r.method === 'DELETE' && r.path.endsWith(`/${skip.vmName}-nic`)), 'skip: the NIC was not deleted');

  const uncoded = await placement('uncoded', { ...deleted(404, refusal('NotFound', 'gone')),
    vmCreate: answer => answer(500, refusal(null, 'the platform failed')) });
  assert.ok(exitedRefusing(uncoded.tick), `uncoded: a refusal with no code was dropped: ${uncoded.tick.stdout}`);
  assert.ok(uncoded.tick.stderr.includes(`no Azure location placed ${uncoded.vmName}: ${WESTUS}: VM create: `), uncoded.tick.stderr);
  assert.ok(uncoded.tick.stderr.includes('the platform failed'), uncoded.tick.stderr);
  assert.ok(!uncoded.tick.stderr.includes(`${NO_ROOM}: NIC create`), `uncoded: a skip was named as a fault: ${uncoded.tick.stderr}`);

  const held = await placement('nic-409', { ...deleted(409, refusal('NicInUse', 'the NIC is still attached')),
    vmCreate: answer => answer(409, refusal('SkuNotAvailable', `${SIZE} is not offered in ${WESTUS}`)) });
  assert.ok(exitedRefusing(held.tick), `nic-409: a NIC left behind was not refused: ${held.tick.stdout}`);
  for (const named of [`Azure VM ${held.vmName} failed in ${WESTUS} (`, 'SkuNotAvailable', 'its NIC could not be removed (', 'NicInUse']) {
    assert.ok(held.tick.stderr.includes(named), `nic-409: the refusal does not name ${named}: ${held.tick.stderr}`);
  }

  // The operation's poll is held until the second tick has been refused.
  let polled;
  let release;
  const heldPoll = new Promise(resume => { release = resume; });
  const lro = await placement('lro', { ...deleted(404, refusal('NotFound', 'gone')),
    vmCreate: (answer, origin) => answer(201, {}, { 'azure-asyncoperation': `${origin}/operations/vm` }),
    beside: new Promise(asked => { polled = asked; }),
    operation: answered => { polled(); heldPoll.then(answered); },
    release: () => release() });
  assert.ok(exitedCleanly(lro.tick), `lro: Failed with SkuNotAvailable was not read as that code: ${lro.tick.stderr}`);
  assert.ok(lro.requests.some(r => r.path === '/operations/vm'), 'lro: the operation was not polled');
  assert.ok(exitedRefusing(lro.beside), `beside: a second tick ran beside the first: ${lro.beside.stdout}`);
  const firstPid = report.commands.find(command => command.case === 'lro' && command.args[0] === 'scheduler')?.pid;
  for (const named of ['another tick holds this store: pid ', 'state/coordinator/tick-lock.json']) {
    assert.ok(lro.beside.stderr.includes(named), `beside: the refusal does not name ${named}: ${lro.beside.stderr}`);
  }
  assert.ok(firstPid && lro.beside.stderr.includes(`pid ${firstPid} on `), `beside: the refusal does not name the running tick's pid ${firstPid}: ${lro.beside.stderr}`);
  assert.equal(lro.lockLeft, false, 'the first tick left its lock behind');
  report.result = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, 2));
  chmodSync(join(output, 'report.json'), 0o600);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
