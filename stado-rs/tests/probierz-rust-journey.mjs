import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, createReadStream } from 'node:fs';
import { chmod, copyFile, mkdir, readFile, stat, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const crate = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repository = resolve(crate, '..');
const compilationBudgetMs = 70 * 60 * 1000;
const executionBudgetMs = 10 * 60 * 1000;
const killGraceMs = 2 * 1000;
const processOutputEncoding = 'utf8';
const testArgs = ['--ignored', '--nocapture', '--test-threads=1'];
const profileEnvironment = {
  CARGO_PROFILE_TEST_DEBUG: '0',
  CARGO_INCREMENTAL: '0',
};
const activeChildren = new Set();

function errorRecord(error) {
  if (!error) return null;
  return {
    name: error.name || null,
    message: error.message || String(error),
    code: error.code ?? null,
    errno: error.errno ?? null,
    syscall: error.syscall ?? null,
    path: error.path ?? null,
  };
}

function signalProcessTree(child, signal) {
  if (!child.pid) return null;
  try {
    if (process.platform === 'win32') child.kill(signal);
    else process.kill(-child.pid, signal);
    return null;
  } catch (error) {
    return error.code === 'ESRCH' ? null : error;
  }
}

function terminateActiveChildren(signal) {
  for (const child of activeChildren) signalProcessTree(child, signal);
}

for (const [signal, exitCode] of [['SIGINT', 130], ['SIGTERM', 143], ['SIGHUP', 129]]) {
  process.once(signal, () => {
    terminateActiveChildren('SIGTERM');
    setTimeout(() => {
      terminateActiveChildren('SIGKILL');
      process.exit(exitCode);
    }, killGraceMs);
  });
}

function runProcess(executable, args, { cwd, env, timeoutMs }) {
  return new Promise((complete) => {
    const startedAt = new Date().toISOString();
    const startedMs = Date.now();
    const child = spawn(executable, args, {
      cwd,
      env,
      detached: process.platform !== 'win32',
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    activeChildren.add(child);
    child.stdout.setEncoding(processOutputEncoding);
    child.stderr.setEncoding(processOutputEncoding);

    let stdout = '';
    let stderr = '';
    let spawnError = null;
    let closeResult = null;
    let timedOut = false;
    let hardKillSent = false;
    let settled = false;
    let hardKillTimer = null;

    child.stdout.on('data', (chunk) => { stdout += chunk; });
    child.stderr.on('data', (chunk) => { stderr += chunk; });
    child.on('error', (error) => { spawnError = error; });

    const finish = () => {
      if (settled || closeResult === null) return;
      if (timedOut && !hardKillSent) return;
      settled = true;
      activeChildren.delete(child);
      clearTimeout(timeoutTimer);
      clearTimeout(hardKillTimer);
      complete({
        executable,
        args,
        cwd,
        startedAt,
        completedAt: new Date().toISOString(),
        durationMs: Date.now() - startedMs,
        timeoutMs,
        exitCode: closeResult.exitCode,
        signal: closeResult.signal,
        timedOut,
        killed: Boolean(child.killed || timedOut),
        spawnError,
        stdout,
        stderr,
      });
    };

    child.on('close', (exitCode, signal) => {
      closeResult = { exitCode, signal };
      finish();
    });

    const timeoutTimer = setTimeout(() => {
      timedOut = true;
      spawnError ||= signalProcessTree(child, 'SIGTERM');
      hardKillTimer = setTimeout(() => {
        spawnError ||= signalProcessTree(child, 'SIGKILL');
        hardKillSent = true;
        finish();
      }, killGraceMs);
    }, timeoutMs);
  });
}

function succeeded(result) {
  return !result.spawnError && !result.timedOut && result.exitCode === 0;
}

function digestText(text) {
  return createHash('sha256').update(text).digest('hex');
}

async function digestFile(file) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest('hex');
}

async function writeProcessRecord(result, artifacts, stem) {
  const stdoutPath = join(artifacts, `${stem}.stdout.log`);
  const stderrPath = join(artifacts, `${stem}.stderr.log`);
  await Promise.all([
    writeFile(stdoutPath, result.stdout, { mode: 0o600 }),
    writeFile(stderrPath, result.stderr, { mode: 0o600 }),
  ]);
  return {
    executable: result.executable,
    args: result.args,
    cwd: result.cwd,
    startedAt: result.startedAt,
    completedAt: result.completedAt,
    durationMs: result.durationMs,
    timeoutMs: result.timeoutMs,
    exitCode: result.exitCode,
    signal: result.signal,
    timedOut: result.timedOut,
    killed: result.killed,
    spawnError: errorRecord(result.spawnError),
    stdout: {
      file: stdoutPath,
      bytes: Buffer.byteLength(result.stdout),
      sha256: digestText(result.stdout),
    },
    stderr: {
      file: stderrPath,
      bytes: Buffer.byteLength(result.stderr),
      sha256: digestText(result.stderr),
    },
  };
}

async function sourceIdentity(checkpoint) {
  const options = { cwd: repository, env: process.env, timeoutMs: 30 * 1000 };
  const [revision, status] = await Promise.all([
    runProcess('git', ['rev-parse', 'HEAD'], options),
    runProcess('git', ['status', '--porcelain', '--untracked-files=all'], options),
  ]);
  if (!succeeded(revision)) {
    throw new Error(`cannot read source revision at ${checkpoint}: ${revision.stderr || revision.spawnError?.message || revision.signal || revision.exitCode}`);
  }
  if (!succeeded(status)) {
    throw new Error(`cannot read source status at ${checkpoint}: ${status.stderr || status.spawnError?.message || status.signal || status.exitCode}`);
  }
  return {
    checkpoint,
    observedAt: new Date().toISOString(),
    revision: revision.stdout.trim(),
    clean: status.stdout.trim().length === 0,
  };
}

function parseCompilerArtifacts(stdout) {
  const artifacts = [];
  for (const line of stdout.split('\n')) {
    if (!line.trim()) continue;
    let message;
    try {
      message = JSON.parse(line);
    } catch (error) {
      throw new Error(`Cargo emitted non-JSON output with --message-format=json: ${error.message}`);
    }
    if (message.reason === 'compiler-artifact' && message.executable) artifacts.push(message);
  }
  return artifacts;
}

function uniqueExecutableArtifacts(artifacts) {
  return [...new Map(artifacts.map((artifact) => [artifact.executable, artifact])).values()];
}

function selectTestArtifact(artifacts, target) {
  const candidates = uniqueExecutableArtifacts(artifacts.filter((artifact) => (
    artifact.target?.name === target && artifact.target?.kind?.includes('test')
  )));
  if (candidates.length !== 1) {
    throw new Error(`Cargo emitted ${candidates.length} executable compiler artifacts for test target ${target}`);
  }
  return candidates[0];
}

function selectStadoArtifact(artifacts) {
  const candidates = uniqueExecutableArtifacts(artifacts.filter((artifact) => (
    artifact.target?.name === 'stado' && artifact.target?.kind?.includes('bin')
  )));
  const productCandidates = candidates.filter((artifact) => artifact.profile?.test === false);
  const selected = productCandidates.length > 0 ? productCandidates : candidates;
  if (selected.length !== 1) {
    throw new Error(`Cargo emitted ${selected.length} usable executable compiler artifacts for the Stado CLI`);
  }
  return selected[0];
}

async function snapshotExecutable(artifact, destination, role) {
  const source = await stat(artifact.executable);
  if (!source.isFile()) throw new Error(`Cargo artifact is not a file: ${artifact.executable}`);
  await copyFile(artifact.executable, destination, constants.COPYFILE_FICLONE);
  await chmod(destination, source.mode & 0o777);
  const [sourceSha256, snapshotSha256, snapshot] = await Promise.all([
    digestFile(artifact.executable),
    digestFile(destination),
    stat(destination),
  ]);
  if (sourceSha256 !== snapshotSha256 || source.size !== snapshot.size) {
    throw new Error(`retained ${role} snapshot differs from Cargo artifact ${artifact.executable}`);
  }
  return {
    role,
    target: artifact.target.name,
    packageId: artifact.package_id,
    targetKind: artifact.target.kind,
    profile: artifact.profile,
    compilerArtifact: artifact.executable,
    snapshot: {
      file: destination,
      bytes: snapshot.size,
      sha256: snapshotSha256,
      mode: `0${(snapshot.mode & 0o777).toString(8)}`,
      copyFlag: 'COPYFILE_FICLONE',
    },
  };
}

function escapeRegExp(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function formatFailure(result) {
  if (!result) return 'not run';
  if (result.spawnError) return result.spawnError.message;
  if (result.timedOut) return `timed out after ${result.timeoutMs}ms`;
  return `exit ${result.exitCode ?? 'unknown'}${result.signal ? ` (${result.signal})` : ''}`;
}

export async function runRecordedRustJourney({
  journey,
  artifactStem,
  targets,
  tests,
  productionMutations,
  contracts,
}) {
  const artifacts = process.env.PROBIERZ_ARTIFACTS;
  const mediaManifest = process.env.PROBIERZ_MEDIA_MANIFEST;
  if (!artifacts) throw new Error('PROBIERZ_ARTIFACTS is required');
  if (!mediaManifest) throw new Error('PROBIERZ_MEDIA_MANIFEST is required');
  await mkdir(artifacts, { recursive: true, mode: 0o700 });

  const failures = [];
  const source = { repository, checkpoints: [], stable: false };
  let compilation = null;
  let compilerArtifacts = [];
  let executableSnapshots = [];
  const executions = [];

  try {
    const identity = await sourceIdentity('before-compilation');
    source.checkpoints.push(identity);
    if (!identity.clean) failures.push('source checkout was not clean before compilation');
  } catch (error) {
    failures.push(error.message);
  }

  if (failures.length === 0) {
    const args = [
      'test', '--locked', '--no-run', '--message-format=json',
      ...targets.flatMap((target) => ['--test', target]),
    ];
    const result = await runProcess('cargo', args, {
      cwd: crate,
      env: { ...process.env, ...profileEnvironment },
      timeoutMs: compilationBudgetMs,
    });
    compilation = await writeProcessRecord(result, artifacts, `${artifactStem}.compilation`);
    compilation.environment = profileEnvironment;
    process.stderr.write(result.stderr);
    if (!succeeded(result)) failures.push(`compilation ${formatFailure(result)}`);
    if (succeeded(result)) {
      try {
        const messages = parseCompilerArtifacts(result.stdout);
        compilerArtifacts = [
          selectStadoArtifact(messages),
          ...targets.map((target) => selectTestArtifact(messages, target)),
        ];
      } catch (error) {
        failures.push(error.message);
      }
    }
  }

  try {
    const identity = await sourceIdentity('after-compilation');
    source.checkpoints.push(identity);
  } catch (error) {
    failures.push(error.message);
  }

  if (failures.length === 0) {
    try {
      const snapshotDirectory = join(artifacts, `${artifactStem}.executables`);
      await mkdir(snapshotDirectory, { recursive: true, mode: 0o700 });
      const stadoArtifact = compilerArtifacts[0];
      executableSnapshots.push(await snapshotExecutable(
        stadoArtifact,
        join(snapshotDirectory, 'stado'),
        'stado-cli',
      ));
      for (const artifact of compilerArtifacts.slice(1)) {
        executableSnapshots.push(await snapshotExecutable(
          artifact,
          join(snapshotDirectory, `${artifact.target.name}.test`),
          'test-executable',
        ));
      }
    } catch (error) {
      failures.push(error.message);
    }
  }

  const executionStartedMs = Date.now();
  if (failures.length === 0) {
    const stado = executableSnapshots.find((artifact) => artifact.role === 'stado-cli');
    for (const target of targets) {
      const testExecutable = executableSnapshots.find((artifact) => (
        artifact.role === 'test-executable' && artifact.target === target
      ));
      const remainingMs = executionBudgetMs - (Date.now() - executionStartedMs);
      if (!testExecutable || remainingMs <= 0) {
        executions.push({
          target,
          status: 'not-run',
          reason: testExecutable ? 'total execution budget exhausted' : 'retained test executable missing',
        });
        failures.push(`${target} was not executed`);
        continue;
      }
      const result = await runProcess(testExecutable.snapshot.file, testArgs, {
        cwd: crate,
        env: {
          ...process.env,
          ...profileEnvironment,
          STADO_TEST_BINARY: stado.snapshot.file,
        },
        timeoutMs: remainingMs,
      });
      const processRecord = await writeProcessRecord(
        result,
        artifacts,
        `${artifactStem}.execution.${target}`,
      );
      processRecord.environment = {
        ...profileEnvironment,
        STADO_TEST_BINARY: stado.snapshot.file,
      };
      executions.push({ target, status: succeeded(result) ? 'completed' : 'failed', process: processRecord });
      process.stdout.write(result.stdout);
      process.stderr.write(result.stderr);
      if (!succeeded(result)) failures.push(`${target} execution ${formatFailure(result)}`);
    }
  }
  const executionDurationMs = Date.now() - executionStartedMs;

  try {
    const identity = await sourceIdentity('after-execution');
    source.checkpoints.push(identity);
  } catch (error) {
    failures.push(error.message);
  }

  if (source.checkpoints.length === 3) {
    const revision = source.checkpoints[0].revision;
    source.revision = revision;
    source.stable = source.checkpoints.every((identity) => identity.clean && identity.revision === revision);
  }
  if (!source.stable) failures.push('source checkout did not remain clean at one revision throughout the journey');

  const combinedStdout = (await Promise.all(executions
    .filter((execution) => execution.process)
    .map((execution) => readFile(execution.process.stdout.file, processOutputEncoding))))
    .join('\n');
  for (const test of tests) {
    if (!new RegExp(`test ${escapeRegExp(test)} \\.\\.\\. ok`).test(combinedStdout)) {
      failures.push(`missing passing Rust test result for ${test}`);
    }
  }

  const tracePath = join(artifacts, `${artifactStem}.trace.json`);
  const trace = {
    schemaVersion: 2,
    kind: 'probierz-stado-cli-trace',
    journey,
    runId: process.env.PROBIERZ_RUN_ID || null,
    status: failures.length === 0 ? 'completed' : 'failed',
    source,
    profileEnvironment,
    phases: {
      compilation: {
        budgetMs: compilationBudgetMs,
        process: compilation,
      },
      execution: {
        budgetMs: executionBudgetMs,
        durationMs: executionDurationMs,
        args: testArgs,
        processes: executions,
      },
    },
    executables: executableSnapshots,
    tests,
    productionMutations,
    contracts,
    failures,
  };
  await writeFile(tracePath, `${JSON.stringify(trace, null, 2)}\n`, { mode: 0o600 });
  await mkdir(dirname(mediaManifest), { recursive: true, mode: 0o700 });
  await writeFile(
    mediaManifest,
    `${JSON.stringify([{ file: tracePath, kind: 'trace', contentType: 'application/json' }], null, 2)}\n`,
    { mode: 0o600 },
  );

  if (failures.length > 0) throw new Error(`${journey} journey failed: ${failures.join('; ')}`);
}
