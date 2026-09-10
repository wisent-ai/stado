// The recorded Rust journey every Probierz spec runs: build the crate, run
// the named tests against the built binary, and write the report and the
// retained artifacts that stand for the run.

import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  parseCompilerArtifacts,
  selectStadoArtifact,
  selectTestArtifact,
  snapshotExecutable,
  uniqueExecutableArtifacts,
} from './journey/artifacts.mjs';
import { escapeRegExp, formatFailure, runProcess, succeeded } from './journey/process.mjs';
import { digestText, sourceIdentity, writeProcessRecord } from './journey/records.mjs';

const crate = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repository = resolve(crate, '..');
const processOutputEncoding = 'utf8';
const compilationBudgetMs = 70 * 60 * 1000;
const executionBudgetMs = 10 * 60 * 1000;
const testArgs = ['--ignored', '--nocapture', '--test-threads=1'];
const profileEnvironment = {
  CARGO_PROFILE_TEST_DEBUG: '0',
  CARGO_INCREMENTAL: '0',
};



export async function runRecordedRustJourney({
  journey,
  artifactStem,
  targets,
  tests,
  productionMutations,
  contracts,
  release = false,
  testFilter = null,
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
      ...(release ? ['--release'] : []),
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
    if (!identity.clean || identity.revision !== source.checkpoints[0]?.revision) {
      failures.push('source checkout changed during compilation; refusing test execution');
    }
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
      const selectedTestArgs = testFilter ? [...testArgs, '--exact', testFilter] : testArgs;
      const result = await runProcess(testExecutable.snapshot.file, selectedTestArgs, {
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
    schemaVersion: 1,
    kind: 'probierz-stado-cli-trace',
    journey,
    runId: process.env.PROBIERZ_RUN_ID || null,
    status: failures.length === 0 ? 'completed' : 'failed',
    completedAt: new Date().toISOString(),
    observation: { reply: combinedStdout },
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
        args: testFilter ? [...testArgs, '--exact', testFilter] : testArgs,
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
