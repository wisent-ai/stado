import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const crate = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
assert.ok(
  process.env.STADO_MOBILE_EGRESS_INTERFACE,
  'STADO_MOBILE_EGRESS_INTERFACE must name the trusted phone tether on the selected Stado host',
);

let stdout;
let stderr;
try {
  ({ stdout, stderr } = await exec('cargo', [
    'test', '--test', 'egress',
    'mobile_egress_uses_the_phone_interface_and_public_ip_is_mobile',
    '--', '--ignored', '--nocapture', '--test-threads=1',
  ], {
    cwd: crate,
    env: { ...process.env },
    timeout: 20 * 60 * 1000,
    maxBuffer: 4 * 1024 * 1024,
  }));
} catch (error) {
  const output = `${error.stdout || ''}\n${error.stderr || ''}`.trim();
  process.stderr.write(`${output}\n`);
  throw new Error(`mobile egress journey failed with exit code ${error.code ?? 'unknown'}`);
}

assert.equal(stderr.includes('FAILED'), false, stderr);
assert.match(stdout, /mobile_egress_uses_the_phone_interface_and_public_ip_is_mobile \.\.\. ok/);
assert.match(stdout, /mobile egress verified: interface=.+; public_ip=.+; isp=.+; country=.+/);
assert.ok(stdout.includes('test result: ok. 1 passed; 0 failed'));
process.stdout.write(stdout);
