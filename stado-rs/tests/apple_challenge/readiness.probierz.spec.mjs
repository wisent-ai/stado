import { runRecordedRustJourney } from '../probierz-rust-journey.mjs';

const story = 'apple_readiness_observes_the_registered_host_without_preparing_it';
await runRecordedRustJourney({
  journey: 'apple-challenge-readiness',
  artifactStem: 'stado-apple-readiness',
  targets: ['apple_challenge'],
  tests: [story],
  testFilter: story,
  release: true,
  productionMutations: 'none: only host gui-automation status is invoked',
  contracts: [
    'the registered host already holds the real version 2 Apple helper',
    'the helper has Accessibility and passes its prompt-free preflight in the declared Aqua session',
    'inspection never prepares the helper, starts Apple sign-in, opens a consent window, or sends a notification',
  ],
});
