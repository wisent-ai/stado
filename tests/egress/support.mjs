import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { createConnection, createServer } from 'node:net';

export function command(report, binary, args, options) {
  const result = spawnSync(binary, args, { ...options, encoding: 'utf8' });
  report.commands.push({ binary, args, status: result.status, signal: result.signal,
    error: result.error?.message, stdout: result.stdout, stderr: result.stderr });
  return result;
}

export async function proxy(report, binary, args, options) {
  const child = spawn(binary, args, options);
  const record = { binary, args, stdout: '', stderr: '' };
  report.commands.push(record);
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', text => { record.stderr += text; });
  const ended = new Promise(resolve => child.once('close', (status, signal) => {
    Object.assign(record, { status, signal });
    resolve();
  }));
  const stop = async () => { child.kill('SIGTERM'); await ended; };
  let ready = false;
  try {
    const address = await new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('exit', () => reject(new Error(`proxy exited before readiness: ${record.stderr}`)));
      child.stdout.on('data', text => {
        record.stdout += text;
        const match = record.stdout.match(/mobile egress ready: (?<address>http:\/\/\S+) via /);
        if (match) resolve(new URL(match.groups.address));
      });
    });
    ready = true;
    return { address, stop };
  } finally {
    if (!ready) await stop();
  }
}

export async function receiver(host) {
  const connections = [];
  const sockets = new Set();
  const server = createServer({ allowHalfOpen: true }, socket => {
    sockets.add(socket);
    const chunks = [];
    const record = { peer: socket.remoteAddress };
    connections.push(record);
    socket.on('data', chunk => chunks.push(chunk));
    socket.on('error', error => { record.error = error.message; });
    socket.on('end', () => {
      record.bytes = Buffer.concat(chunks);
      socket.end();
    });
    socket.on('close', () => sockets.delete(socket));
  });
  server.listen({ host });
  await once(server, 'listening');
  return { address: server.address(), connections, async close() {
    for (const socket of sockets) socket.destroy();
    const closed = once(server, 'close');
    server.close();
    await closed;
  } };
}

export async function transmit(address, pieces) {
  const socket = createConnection({ host: address.hostname, port: address.port });
  const received = [];
  const errors = [];
  socket.on('error', error => errors.push(error.message));
  socket.on('data', chunk => received.push(chunk));
  const closed = new Promise(resolve => socket.once('close', resolve));
  await once(socket, 'connect');
  for (const piece of pieces) {
    await new Promise((resolve, reject) => socket.write(piece, error => error ? reject(error) : resolve()));
  }
  socket.end();
  await closed;
  return { bytes: Buffer.concat(received), errors };
}
