// Real Stado Desktop journey for the Hosts screen's live resource contract.
//
// Probierz supplies the CUA transport. Every assertion and product action lives
// here with Stado: the screen must show live capacity without fixed slot counts,
// and disk reclamation must remain refused until its reason is supplied.
import assert from 'node:assert/strict';

export function runHostsDynamicCapacityJourney({
  activate,
  assertAbsent,
  assertField,
  assertRefusedControl,
  attempt,
  createEvidence,
  dumpWindows,
  launchConsole,
  openScreen,
  quitApp,
  selectRow,
  waitForScreen,
}) {
  const gatesMs = 180_000;
  const sheetMs = 120_000;
  const evidence = createEvidence('stado-hosts-screen');
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
    const loaded = evidence.capture(app.pid, app.windowId, 'loaded');

    assertAbsent(
      loaded,
      [
        'No host inventory',
        'No registered hosts',
        'No hosts in this filter',
        'Reading host capacity reports',
        'No host selected',
      ],
      'the Hosts screen did not load real fleet state',
    );

    const claiming = assertField(loaded, 'Claiming work', { pattern: /^(Yes|No)$/ });
    const blockers = assertField(loaded, 'Blockers');
    assert.notEqual(
      blockers,
      'Reading…',
      `${row.label} renders a spinner where its blockers belong`,
    );
    if (claiming === 'No') {
      assert.ok(
        loaded.tree.includes('This host is claiming no work'),
        `a host that claims nothing must say so; tree: ${loaded.tree.slice(-2000)}`,
      );
    }

    assertField(loaded, 'Free space', {
      pattern: /^([\d.,]+ GB free|Not reported)/,
    });
    assertField(loaded, 'Cleanup policy mode');
    assert.match(loaded.tree, /[\d.,]+ GB/, 'the screen renders no disk figure for any host');

    assertField(loaded, 'Running jobs', { pattern: /^(\d+|Not reported)$/ });
    assertField(loaded, 'CPU', {
      pattern: /^(\d+ available of \d+ cores|Not reported)$/,
    });
    assertField(loaded, 'RAM', {
      pattern: /^([\d.,]+ free of [\d.,]+ GB|Not reported)$/,
    });
    assertField(loaded, 'Accelerators available');
    assertField(loaded, 'VRAM', {
      pattern: /^([\d.,]+ free of [\d.,]+ GB|Not reported)$/,
    });
    assertAbsent(
      loaded,
      [/\b(?:FREE )?SLOTS?\b/i],
      'the Hosts screen retained the removed fixed-capacity design',
    );

    const dialog = activate(app.pid, app.windowId, 'Reclaim disk…', {
      needle: 'Reclaim disk on ',
      timeoutMs: sheetMs,
    });
    const sheet = dialog.view;
    assert.ok(
      sheet.tree.includes('Why this host needs the space'),
      `the reclamation sheet does not ask why; tree: ${sheet.tree.slice(-2000)}`,
    );
    const refusal = [
      'Type a reason to enable the apply.',
      'The apply stays unavailable until the dry run above has answered for',
    ].find((text) => sheet.tree.includes(text));
    assert.ok(
      refusal,
      `the sheet does not state why the apply is unavailable; tree: ${sheet.tree.slice(-2500)}`,
    );
    assert.match(
      sheet.tree,
      /stado host reclaim .*--apply --reason "why this host needs the space" --json/,
      'the sheet does not show the apply command with an unfilled reason',
    );

    const control = assertRefusedControl(sheet, 'Reclaim now');
    const refused = evidence.capture(app.pid, dialog.windowId, 'refused-without-reason');
    assert.ok(
      refused.tree.includes('Why this host needs the space'),
      `the sheet stopped asking why; tree: ${refused.tree.slice(-2000)}`,
    );
    assert.equal(
      assertRefusedControl(refused, 'Reclaim now'),
      control,
      'the apply became reachable without a reason being typed',
    );
    assertAbsent(
      refused,
      [
        'What reclamation freed',
        'Reclaiming disk on ',
        'The pass ran and reported no stages',
        'A reason is required',
      ],
      'the screen applied a reclamation without a typed reason',
    );
    assert.match(
      refused.tree,
      /--apply --reason "why this host needs the space" --json/,
      'the refused sheet no longer shows the unfilled apply command',
    );

    attempt(app.pid, dialog.windowId, 'Cancel');
    const after = waitForScreen(app.pid, app.windowId, /CLEANUP POLICY MODE/, sheetMs);
    assertAbsent(
      after,
      ['What reclamation freed', 'Reclaim disk on '],
      'the reclamation sheet outlived a cancel',
    );
  } finally {
    try {
      dumpWindows(app.pid, 'stado-hosts-screen');
      evidence.write();
    } finally {
      quitApp(app.pid);
    }
  }
}
