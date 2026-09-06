import { strict as assert } from 'node:assert';
import { runRecordedRustJourney } from '../probierz-rust-journey.mjs';

assert.equal(
  process.platform,
  'darwin',
  'native-readers requires the dedicated macOS host selected by Stado',
);

await runRecordedRustJourney({
  journey: 'native-readers',
  artifactStem: 'stado-native-readers',
  targets: ['native_readers'],
  tests: [
    'convergence_reloads_a_cached_private_stado_definition_once',
    'service_update_reloads_a_cached_global_stado_definition_once',
  ],
  executionBudgetMs: 15 * 60 * 1000,
  productionMutations: 'one collision-resistant Probierz LaunchAgent in the selected macOS login domain; isolated HOME, storage, registry, port, logs, and binaries; removed through Stado service bootout and guarded host remove-file lifecycle commands',
  contracts: [
    'a real launchd unit can keep executing a private Stado file after its on-disk plist changes to the delivered root',
    'release converge-local-readers reloads that changed definition through the exact launchd domain observed to own it',
    'the public service label-print readback proves the replacement device, inode, executable path, and SHA-256 equal the delivered root file before convergence succeeds',
    'repeating convergence leaves an already-correct process running under the same pid',
    'service update installs the real archive into a private tree, reloads the cached global definition, proves its replacement image, and leaves it running on an identical replay',
    'an incompatible archive is refused while the current symlink, plist bytes, live pid and mapped private image remain unchanged',
  ],
});
