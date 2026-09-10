// Which binaries a build actually produced, which of them the journey runs,
// and the copy it keeps beside the report so the run can be re-read against
// the exact executable that produced it.

import { constants } from 'node:fs';
import { chmod, copyFile, stat } from 'node:fs/promises';

import { digestFile } from './records.mjs';

export function parseCompilerArtifacts(stdout) {
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

export function uniqueExecutableArtifacts(artifacts) {
  return [...new Map(artifacts.map((artifact) => [artifact.executable, artifact])).values()];
}

export function selectTestArtifact(artifacts, target) {
  const candidates = uniqueExecutableArtifacts(artifacts.filter((artifact) => (
    artifact.target?.name === target && artifact.target?.kind?.includes('test')
  )));
  if (candidates.length !== 1) {
    throw new Error(`Cargo emitted ${candidates.length} executable compiler artifacts for test target ${target}`);
  }
  return candidates[0];
}

export function selectStadoArtifact(artifacts) {
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

export async function snapshotExecutable(artifact, destination, role) {
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
