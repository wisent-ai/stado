import { runRecordedRustJourney } from '../probierz-rust-journey.mjs';

const story = 'apple_readiness_observes_the_registered_host_without_preparing_it';
await runRecordedRustJourney({
  journey: 'apple-challenge-readiness',
  artifactStem: 'stado-apple-readiness',
  targets: ['apple_challenge'],
  tests: [story],
  testFilter: story,
  release: true,
  productionMutations: 'none on the registered host: CLI and native API readiness reads, plus an unconfirmed request for an unknown host',
  contracts: [
    'the registered host already holds the real version 2 Apple helper',
    'the helper has Accessibility and passes its prompt-free preflight in the declared Aqua session',
    'the real native API returns the same readiness state and refuses mutation without confirmation before resolving the unknown host',
    'inspection never prepares the helper, starts Apple sign-in, opens a consent window, or sends a notification',
  ],
});
