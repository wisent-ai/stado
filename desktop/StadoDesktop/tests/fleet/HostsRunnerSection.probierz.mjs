// Real Stado Desktop journey for the host runner section.
//
// Probierz supplies the CUA transport; every assertion and product action lives
// here with Stado. What it defends: Desktop shows the same runner lifecycle the
// CLI has, and the two facts that used to exist only in a terminal — which
// GitHub door the registration went through, and whether a job is holding this
// machine's one job slot.
//
// The gap this closes: `host precheck-runner` gained `--repository` and a host
// job gate on 2026-09-06 and Desktop had neither, so an operator could read a
// host's disk, services, gates and Apple readiness on this screen and had to
// leave it for the one lifecycle that had just changed.
import assert from 'node:assert/strict';

export function runHostsRunnerSectionJourney({
  assertAbsent,
  assertField,
  createEvidence,
  launchConsole,
  openScreen,
  quitApp,
  selectRow,
}) {
  const gatesMs = 180_000;
  const evidence = createEvidence('stado-hosts-runner');
  const app = launchConsole();

  try {
    openScreen(app.pid, app.windowId, 'Hosts', {
      loaded: /AX\w*Button \(All hosts/,
      failures: ['No host inventory', 'No registered hosts'],
      refresh: 'Refresh',
      timeoutMs: gatesMs,
    });

    const { row } = selectRow(app.pid, app.windowId, {
      needle: /CLEANUP POLICY MODE/,
      timeoutMs: 60_000,
    });
    const loaded = evidence.capture(app.pid, app.windowId, 'runner-section');

    assert.ok(
      loaded.tree.includes('GitHub runner'),
      `${row.label} shows no runner section; tree: ${loaded.tree.slice(-2000)}`,
    );

    // The read-only command, spelled exactly as the CLI is invoked. A screen
    // that shows an action without naming its command hides which host state
    // it is about to change.
    assert.ok(
      loaded.tree.includes('host precheck-runner status'),
      `the runner section names no read-only command; tree: ${loaded.tree.slice(-2000)}`,
    );

    // Before anything is read, every field says so rather than inventing a
    // scope: a runner nobody asked about is not an organization-wide runner.
    assertField(loaded, 'Registration scope', { pattern: /^(Not read|organization:|repository:|unrecorded)/ });
    assertField(loaded, 'Host job slot', { pattern: /^(Not read|none|unknown|[\w.-]+ (pid=\d+|stale))$/ });
    assertField(loaded, 'Labels');

    // Every lifecycle verb the CLI has is reachable here.
    for (const control of [
      'Read runner',
      'Install or reconcile',
      'Restart in place',
      'Remove',
    ]) {
      assert.ok(
        loaded.tree.includes(control),
        `the runner section is missing the ${control} control; tree: ${loaded.tree.slice(-2000)}`,
      );
    }

    // The scope is typed, not guessed: the field exists and is empty, which is
    // what the CLI's absent `--repository` means.
    assert.ok(
      loaded.tree.includes('Repository (optional)'),
      `the runner section offers no registration scope; tree: ${loaded.tree.slice(-2000)}`,
    );

    assertAbsent(
      loaded,
      ['Reading…'],
      'the runner section renders a spinner where its facts belong',
    );

    return { row: row.label, evidence: evidence.manifest() };
  } finally {
    quitApp(app.pid);
  }
}
