import { strict as assert } from 'node:assert';
import { runRecordedRustJourney } from '../probierz-rust-journey.mjs';

assert.equal(
  process.platform,
  'darwin',
  'native-readers requires the dedicated macOS host selected by Stado',
);

await runRecordedRustJourney({
  journey: 'native-readers',
  runId: process.env.PROBIERZ_RUN_ID || null,
  status: exitCode === 0 ? 'completed' : 'failed',
  source: {
    repository,
    revision: revisionResult.stdout.trim(),
    dirty: statusResult.stdout.trim().length > 0,
  },
  process: {
    executable: 'cargo',
    args,
    cwd: crate,
    exitCode,
    signal,
    killed: Boolean(result.error?.killed),
    stdout: {
      file: stdoutPath,
      bytes: Buffer.byteLength(result.stdout),
      sha256: sha256(result.stdout),
    },
    stderr: {
      file: stderrPath,
      bytes: Buffer.byteLength(result.stderr),
      sha256: sha256(result.stderr),
    },
  },
  tests,
  productionMutations: 'one collision-resistant Probierz LaunchAgent in the selected macOS login domain; isolated HOME, storage, registry, port, logs, and binaries; removed through Stado service bootout and guarded space file remove lifecycle commands',
  contracts: [
    'a real launchd unit can keep executing a private Stado file after its on-disk plist changes to the delivered root',
    'release converge-local-readers reloads that changed definition through the exact launchd domain observed to own it',
    'the public service label-print readback proves the replacement device, inode, executable path, and SHA-256 equal the delivered root file before convergence succeeds',
    'repeating convergence leaves an already-correct process running under the same pid',
    'service update installs the real archive into a private tree, reloads the cached global definition, proves its replacement image, and leaves it running on an identical replay',
    'an incompatible archive is refused while the current symlink, plist bytes, live pid and mapped private image remain unchanged',
  ],
});
