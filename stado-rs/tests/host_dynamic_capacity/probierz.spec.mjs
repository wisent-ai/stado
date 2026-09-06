import { runRecordedRustJourney } from '../probierz-rust-journey.mjs';

await runRecordedRustJourney({
  journey: 'host-dynamic-capacity',
  artifactStem: 'stado-host-dynamic-capacity',
  targets: ['host_dynamic_capacity', 'capacity'],
  tests: [
    'host_gates_use_live_resources_and_never_fixed_slots',
    'registry_policy_rewrite_removes_legacy_fixed_capacity_declarations',
    'live_resources_admit_two_jobs_despite_legacy_single_worker_limits',
  ],
  productionMutations: 'none: every story uses an isolated local Stado store and the worker runs only its submitted workloads',
  contracts: [
    'host gates reports live CPU, RAM, VRAM, accelerator, and running-job capacity',
    'the public JSON and human output contain no fixed slot count',
    'a paused host is refused with its exact blocker sentence',
    'a registry policy write removes retired fixed worker-cap declarations',
    'a real worker runs both submitted workloads concurrently despite legacy caps of one and persists both completed job records',
  ],
});
