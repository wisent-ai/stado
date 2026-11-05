import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly } from '../native/process.mjs';
import { snapshotSubject, verifyRevision } from '../native/subject.mjs';

// `stado web edge status` and `repair` against real Azure security groups,
// in resource groups this test creates and deletes. Nothing billed is made:
// resource groups, a virtual network, security groups and a network interface
// with no VM and no public address cost nothing.
// 1. A subnet whose security group holds no allow, and an interface
//    `<edge>-nic` in it whose own group allows 80 and 443: `edge status`
//    names the subnet's group as dropping 80 and 443 and exits non-zero, a
//    probe of a documentation address ending at the declared probe_seconds;
// 2. `edge repair <edge>` writes an allow for 80 and 443 into the subnet's
//    group whose destination is the interface's private address, never the
//    subnet, read back with `az`; a second repair writes nothing;
// 3. `edge repair` of a name with no interface is refused naming it;
// 4. a subnet group in another resource group is refused naming that group,
//    and nothing is written to it.
// STADO_BIN selects the Stado under test; AZURE_SUBSCRIPTION_ID and
// STADO_EDGE_TEST_REGION the subscription and region the test creates in.
// The report keeps every command, exit status and output under .build/.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'web-edge-rules-'));
const stado = process.env.STADO_BIN;
const subscription = process.env.AZURE_SUBSCRIPTION_ID;
const region = process.env.STADO_EDGE_TEST_REGION;
const suffix = `${process.pid}`;
const group = `stado-test-edge-rules-${suffix}`;
const foreign = `stado-test-edge-rules-other-${suffix}`;
const edge = `edgetest${suffix}`;
const report = { started_at: new Date().toISOString(), group, foreign, edge, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args, env = process.env) {
  const answer = spawnSync(program, args, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status, signal: answer.signal,
    error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
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
const az = args => JSON.parse(success(run('az', [...args, '--subscription', subscription, '--output', 'json'])) || 'null');
// The edge declaration status reads; 192.0.2.1 is a documentation address
// (RFC 5737), so its probe ends at the declared deadline.
function scoped(resourceGroup) {
  const path = join(output, `config-${resourceGroup}.json`);
  writeFileSync(path, `${JSON.stringify({ web_api: { products: {}, edge: {
    target: edge, address: '192.0.2.1', contact: 'edge-test@example.com', probe_seconds: 2 } } })}\n`);
  return { ...process.env, STADO_CONFIG: path, AZURE_SUBSCRIPTION_ID: subscription, AZURE_RESOURCE_GROUP: resourceGroup };
}
let created = [];
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  if (!stado || !subscription || !region) {
    report.result = 'blocked';
    throw new Error(`requires STADO_BIN, AZURE_SUBSCRIPTION_ID and STADO_EDGE_TEST_REGION; missing: ${[
      !stado && 'STADO_BIN', !subscription && 'AZURE_SUBSCRIPTION_ID', !region && 'STADO_EDGE_TEST_REGION',
    ].filter(Boolean).join(', ')}`);
  }
  report.stado = snapshotSubject(stado, output);
  const binary = report.stado.path;
  report.stado.version = success(run(binary, ['--version']));
  verifyRevision(report.stado.version, report.source_revision);

  for (const name of [group, foreign]) {
    az(['group', 'create', '--name', name, '--location', region]);
    created.push(name);
  }
  az(['network', 'nsg', 'create', '--resource-group', group, '--name', 'subnet-nsg', '--location', region]);
  az(['network', 'vnet', 'create', '--resource-group', group, '--name', 'edge-vnet', '--location', region,
    '--address-prefixes', '10.40.0.0/16', '--subnet-name', 'edge-subnet', '--subnet-prefixes', '10.40.0.0/24']);
  az(['network', 'vnet', 'subnet', 'update', '--resource-group', group, '--vnet-name', 'edge-vnet',
    '--name', 'edge-subnet', '--network-security-group', 'subnet-nsg']);
  az(['network', 'nsg', 'create', '--resource-group', group, '--name', 'nic-nsg', '--location', region]);
  for (const [name, port, priority] of [['allow-http', '80', '300'], ['allow-https', '443', '310']]) {
    az(['network', 'nsg', 'rule', 'create', '--resource-group', group, '--nsg-name', 'nic-nsg', '--name', name,
      '--priority', priority, '--access', 'Allow', '--protocol', 'Tcp', '--direction', 'Inbound',
      '--source-address-prefixes', 'Internet', '--destination-port-ranges', port]);
  }
  const nic = az(['network', 'nic', 'create', '--resource-group', group, '--name', `${edge}-nic`, '--location', region,
    '--vnet-name', 'edge-vnet', '--subnet', 'edge-subnet', '--network-security-group', 'nic-nsg']);
  const address = nic.NewNIC?.ipConfigurations?.[0]?.privateIPAddress ?? nic.ipConfigurations?.[0]?.privateIPAddress;
  assert.ok(address, `the interface has no private address: ${JSON.stringify(nic)}`);
  report.private_address = address;

  // 1. Status names the subnet's group before any probe, and fails.
  const before = run(binary, ['web', 'edge', 'status', '--json'], scoped(group));
  assert.ok(!exitedCleanly(before), 'status succeeded while nothing answers');
  const status = JSON.parse(before.stdout);
  assert.equal(status.network.read, true, `security groups not read: ${JSON.stringify(status.network)}`);
  const dropping = status.network.dropping.find(layer => layer.group === 'subnet-nsg');
  assert.deepEqual(dropping?.ports, [80, 443], `subnet-nsg is not named as dropping 80 and 443: ${before.stdout}`);
  assert.ok(status.https.detail.includes('within 2s'), `the probe did not end at probe_seconds: ${status.https.detail}`);

  // 2. Repair allows 80 and 443 to the interface's address only.
  const repaired = JSON.parse(success(run(binary, ['web', 'edge', 'repair', edge, '--json'], scoped(group))));
  assert.equal(repaired.written.length, 2, `repair wrote ${JSON.stringify(repaired.written)}`);
  for (const port of [80, 443]) {
    const rule = az(['network', 'nsg', 'rule', 'show', '--resource-group', group, '--nsg-name', 'subnet-nsg',
      '--name', `stado-${edge}-${port}`]);
    assert.equal(rule.destinationAddressPrefix, address, `the ${port} allow opens ${rule.destinationAddressPrefix}`);
    assert.equal(rule.access, 'Allow');
  }
  const again = JSON.parse(success(run(binary, ['web', 'edge', 'repair', edge, '--json'], scoped(group))));
  assert.deepEqual(again.written, [], `a second repair wrote ${JSON.stringify(again.written)}`);

  // 3. No interface by that name.
  refused(run(binary, ['web', 'edge', 'repair', `${edge}-absent`], scoped(group)), `${edge}-absent-nic`);

  // 4. A subnet group outside AZURE_RESOURCE_GROUP is refused and untouched.
  az(['network', 'nsg', 'create', '--resource-group', foreign, '--name', 'foreign-nsg', '--location', region]);
  const foreignId = az(['network', 'nsg', 'show', '--resource-group', foreign, '--name', 'foreign-nsg']).id;
  az(['network', 'vnet', 'subnet', 'update', '--resource-group', group, '--vnet-name', 'edge-vnet',
    '--name', 'edge-subnet', '--network-security-group', foreignId]);
  refused(run(binary, ['web', 'edge', 'repair', edge], scoped(group)), 'foreign-nsg');
  const untouched = az(['network', 'nsg', 'show', '--resource-group', foreign, '--name', 'foreign-nsg']);
  assert.equal(untouched.securityRules.length, 0, `a rule was written to foreign-nsg: ${JSON.stringify(untouched.securityRules)}`);
  report.result = 'passed';
} catch (error) {
  report.error = String(error);
  throw error;
} finally {
  for (const name of created) {
    const deleted = run('az', ['group', 'delete', '--name', name, '--yes', '--subscription', subscription]);
    const left = run('az', ['group', 'exists', '--name', name, '--subscription', subscription]);
    report.commands.push({ cleanup: name, deleted: deleted.status, exists: left.stdout.trim() });
    if (left.stdout.trim() !== 'false') report.result = 'failed';
  }
  report.finished_at = new Date().toISOString();
  report.scope = 'web edge status and repair against real Azure security groups in resource groups the test creates and deletes';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
