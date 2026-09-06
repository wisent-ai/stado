import { runRecordedRustJourney } from '../probierz-rust-journey.mjs';

await runRecordedRustJourney({
  journey: 'service-convergence',
  artifactStem: 'stado-service-convergence',
  targets: ['service_convergence'],
  tests: ['authenticated_services_api_converges_real_same_host_state'],
  executionBudgetMs: 20 * 60 * 1000,
  productionMutations: 'none: real Stado, storage, registry, grants, binaries and listener are isolated below the source target directory; the real Skarbiec download cache is checksum-pinned runner state',
  contracts: [
    'built Stado provisions the target-local verifier bearer through real Skarbiec without requiring or returning a JSON token',
    'repeated verifier provisioning preserves the exact owner-only bearer bytes and the resulting grant authorizes a real server-side item read',
    'symlink and empty existing bearer paths retain the source-grounded refusal and change neither the vault nor protected fixture state',
    'nonlocal GET and POST authenticate independently through action-scoped existing-registry-client grants stored in real Skarbiec',
    'malformed, unknown and unauthorized requests are refused without opening a host mutation',
    'host-wide and selected-binary reports use the real local hostname and Stado same-host execution rather than SSH',
    'a selected current source-built Stado binary succeeds without redundant delivery',
    'a missing declared Skarbiec release returns HTTP 200 with the complete nonzero convergence envelope and failed release diagnosis',
    'failed delivery preserves the installed source-built Stado, real Skarbiec and protected operator state byte-for-byte',
  ],
});
