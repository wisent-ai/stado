import { runRecordedRustJourney } from '../probierz-rust-journey.mjs';

const story = 'apple_only_preparation_preserves_other_gui_state_and_works_through_the_desktop_api';
await runRecordedRustJourney({
  journey: 'apple-challenge-preparation',
  artifactStem: 'stado-apple-preparation',
  targets: ['apple_challenge'],
  tests: [story],
  testFilter: story,
  release: true,
  productionMutations: 'only the registered Apple helper and its Accessibility grant; no Apple sign-in, notifications, browser, CuaDriver, autologin, or remote-management change',
  contracts: [
    'the real registered host passes the helper preflight after Apple-only preparation',
    'CLI and native API preparation preserve every unrelated observed GUI setting',
    'the native API refuses mutation without confirmation and retains the real host receipt',
  ],
});
