import { runRecordedRustJourney } from '../probierz-rust-journey.mjs';

await runRecordedRustJourney({
  journey: 'release-pipeline',
  artifactStem: 'stado-release-pipeline',
  targets: ['ci-cd'],
  tests: [
    'a_real_release_builds_publishes_and_installs_its_binary',
    'stale_target_capacity_still_enqueues_its_exact_release_delivery',
    'a_cancelled_release_build_is_retried_under_a_new_job',
  ],
  productionMutations: 'none: every story creates an isolated committed product, Stado store, worker, and real Skarbiec vault',
  contracts: [
    'a real worker builds committed source and the signed release installs and executes its binary',
    'a stale delivery target still receives its exact recovery job',
    'a cancelled unfinished build retries under a distinct job identity and installs the completed release',
  ],
});
