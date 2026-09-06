import { runRecordedRustJourney } from '../probierz-rust-journey.mjs';

await runRecordedRustJourney({
  journey: 'run-retention',
  artifactStem: 'stado-run-retention',
  targets: ['run_history'],
  tests: [
    'coordinator_retains_an_unlinked_legacy_terminal_job_from_its_manifest_entry',
    'coordinator_preserves_settled_history_and_refuses_missing_unretired_history',
  ],
  productionMutations: 'none: the product binary uses an isolated local Stado store',
  contracts: [
    'the coordinator retains the exact legacy terminal job named by its manifest entry',
    'the lifecycle blob is reaped only after its outcome is retained',
    'settled cancellation history is not reopened after its run manifest is removed',
    'an unretired transition still refuses a missing run manifest',
  ],
});
