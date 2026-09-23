import { runRecordedRustJourney } from '../probierz-rust-journey.mjs';

await runRecordedRustJourney({
  journey: 'native-build',
  artifactStem: 'stado-native-build',
  targets: ['builds'],
  tests: ['build_recipe_polls_public_git_runs_on_matching_worker_and_publishes_artifact'],
  executionBudgetMs: 20 * 60 * 1000,
  productionMutations: 'none: the real worker and coordinator use an isolated local Stado registry and store',
  contracts: [
    'the coordinator observes a real public Git branch and submits a platform-constrained build',
    'disabling further polling does not cancel the already-submitted build',
    'builds status resolves the completed outcome after coordinator cleanup',
    'stado results downloads the actual build-output.txt bytes after repeated cleanup',
    'malformed clone URLs and duplicate recipe names are refused',
  ],
});
