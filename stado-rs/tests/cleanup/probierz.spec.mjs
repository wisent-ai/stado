import { runRecordedRustJourney } from '../probierz-rust-journey.mjs';

await runRecordedRustJourney({
  journey: 'disk-cleanup',
  artifactStem: 'stado-disk-cleanup',
  targets: ['cleanup'],
  tests: [
    'dry_run_reports_eligible_cache_without_deleting_or_persisting',
    'enforce_deletes_only_tagged_cache_and_persists_reclaimed_progress',
    'overdue_lock_stays_report_only_until_the_predecessor_kernel_lock_is_released',
    'busy_lock_preserves_the_reclaim_hysteresis_and_scan_cursor',
    'once_and_watch_are_refused_with_the_public_usage_sentence',
  ],
  productionMutations: 'none: every story creates a separate temporary home, cache root, and local Stado store',
  contracts: [
    'preview reports an eligible tagged cache without deleting it or persisting janitor state',
    'enforcement deletes only the tagged cache and persists reclaimed progress',
    'an overdue predecessor lock remains report-only until its kernel lock is released',
    'a busy lock preserves reclaim hysteresis and the build-cache scan cursor',
    'the CLI refuses simultaneous --once and --watch with exit code 2 and its public usage sentence',
  ],
});
