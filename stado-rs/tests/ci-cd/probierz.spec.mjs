import { runRecordedRustJourney } from '../probierz-rust-journey.mjs';

await runRecordedRustJourney({
  journey: 'release-pipeline',
  artifactStem: 'stado-release-pipeline',
  targets: ['ci-cd'],
  tests: [
    'a_real_release_builds_publishes_and_installs_its_binary',
    'a_cancelled_release_build_is_retried_under_a_new_job',
  ],
  executionBudgetMs: 60 * 60 * 1000,
  productionMutations: 'none: the worker, registry, release store, real Skarbiec vault and installed product are isolated below the source target directory',
  contracts: [
    'a real worker builds committed source, signs through real Skarbiec, publishes the release and installs its executable',
    'the installed ci-release-probe binary executes and reports its actual version',
    'a cancelled build receives a distinct retry job and that retry installs and executes the real product',
  ],
});
