// A real OpenSSH server for a journey's isolated home, standing for a fleet
// host, which the journey stops with SIGSTOP once it has answered: the kernel
// still completes every TCP connection into the stopped server's backlog and
// nothing ever sends a banner, as on a host whose sshd hangs. The Node
// journeys' counterpart of hung_ssh.rs, with the same files and settings.
//
// The server runs as the account the journey runs as, on a loopback port,
// with its own host key, its own authorized key and a forced HOME of the
// run's home, so a command it runs never reads or writes the account's own
// state. The client side lives in that home too: ~/.ssh/id_ed25519 and a
// known_hosts line for [<loopback>]:<port>, which is how the product trusts
// a server off SSH's own port.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createWriteStream, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

// The OpenSSH server every supported platform installs at this path.
const SSHD = '/usr/sbin/sshd';

function keygen(path) {
  const made = spawnSync('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-C', 'journey', '-f', path],
    { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
  assert.ok(!made.status && !made.error, `ssh-keygen -f ${path}: ${made.stderr || made.error?.message}`);
  return readFileSync(`${path}.pub`, 'utf8').trim();
}

// Serve `home`'s client account on `host`:`port` and resolve once the server
// says it listens there; its own log line is the event waited for.
export async function hungSsh(home, host, port) {
  const directory = join(home, 'sshd');
  const client = join(home, '.ssh');
  mkdirSync(directory, { recursive: true });
  mkdirSync(client, { recursive: true });
  const hostKey = keygen(join(directory, 'host_key'));
  const userKey = keygen(join(client, 'id_ed25519'));
  writeFileSync(join(client, 'known_hosts'), `[${host}]:${port} ${hostKey}\n`);
  writeFileSync(join(directory, 'authorized_keys'),
    `environment="HOME=${home}",environment="PATH=${process.env.PATH}" ${userKey}\n`);
  const config = join(directory, 'sshd_config');
  writeFileSync(config, [
    `Port ${port}`, `ListenAddress ${host}`, `HostKey ${join(directory, 'host_key')}`,
    `PidFile ${join(directory, 'sshd.pid')}`, `AuthorizedKeysFile ${join(directory, 'authorized_keys')}`,
    'PasswordAuthentication no', 'KbdInteractiveAuthentication no', 'UsePAM no', 'StrictModes no',
    'PermitUserEnvironment yes', 'LogLevel INFO', '',
  ].join('\n'));
  const log = join(directory, 'sshd.log');
  const kept = createWriteStream(log);
  const server = spawn(SSHD, ['-D', '-e', '-f', config], { stdio: ['ignore', 'ignore', 'pipe'] });
  const wanted = `Server listening on ${host} port ${port}`;
  const said = [];
  await new Promise((listening, ended) => {
    server.once('error', error => ended(new Error(`start ${SSHD}: ${error.message}`)));
    server.once('exit', (code, signal) =>
      ended(new Error(`the SSH server ended (${code ?? signal}) before it said ${wanted}: ${said.join('\n')}`)));
    createInterface({ input: server.stderr }).on('line', line => {
      kept.write(`${line}\n`);
      said.push(line);
      if (line.includes(wanted)) listening();
    });
  });
  return {
    destination: `[${host}]:${port}`,
    log,
    // Stop the server: from now on it answers nothing it accepts.
    hang() {
      assert.ok(server.kill('SIGSTOP'), `stop the SSH server ${server.pid}`);
    },
    // A stopped server cannot act on the kill until it runs again.
    close() {
      server.removeAllListeners('exit');
      assert.ok(server.kill('SIGCONT'), `resume the SSH server ${server.pid} before ending it`);
      assert.ok(server.kill('SIGKILL'), `end the SSH server ${server.pid}`);
      kept.end();
    },
  };
}

// What a stado command's stderr says when it opened an SSH connection to
// `destination`: the wait it announces before the connection answers.
export function connecting(destination) {
  return `czekam: SSH connection; gdzie: ${destination};`;
}
