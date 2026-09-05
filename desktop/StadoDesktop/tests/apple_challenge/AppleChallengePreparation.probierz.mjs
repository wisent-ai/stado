// Real Stado Desktop journey for prompt-free Apple challenge preparation.
//
// Probierz supplies the already-serving native CUA transport and evidence sink.
// This journey never starts or repairs CuaDriver, never opens a browser, and
// never follows an Apple authentication or system-consent trajectory.
import assert from 'node:assert/strict';

const GATES_MS = 180_000;
const PREPARATION_MS = 360_000;
const APPLE_HELPER_VERSION = '2';

function quoted(argument) {
  if (/^[A-Za-z0-9_\-./:=@+,]+$/u.test(argument)) return argument;
  return `"${argument.replaceAll('\\', '\\\\').replaceAll('"', '\\"')}"`;
}

function preparationCommand(host) {
  return [
    'stado',
    'host',
    'gui-automation',
    'grant-accessibility',
    quoted(host),
    '--apple-only',
    '--json',
  ].join(' ');
}

function escapedRegex(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function reportItemPattern(name, value) {
  return new RegExp(`${escapedRegex(name)}:\\s*${escapedRegex(value)}(?=[\\s"),]|$)`, 'i');
}


function exactRefusal(view) {
  const lines = view.tree.split('\n');
  const marker = lines.findIndex((line) => (
    line.includes('Apple code capture is unavailable')
    || /AX\w*Button \(Dismiss\)/.test(line)
  ));
  const start = marker < 0 ? Math.max(0, lines.length - 30) : Math.max(0, marker - 16);
  const end = marker < 0 ? lines.length : Math.min(lines.length, marker + 8);
  return lines.slice(start, end).map((line) => line.trim()).filter(Boolean).join('\n');
}

function addFailure(current, error, context) {
  const next = error instanceof Error ? error : new Error(String(error));
  if (!current) return next;
  return new AggregateError([current, next], `${current.message}; additionally ${context}: ${next.message}`);
}

export function runAppleChallengePreparationJourney({
  assertField,
  click,
  createEvidence,
  dumpWindows,
  launchConsole,
  openScreen,
  quitApp,
  readPromptFreeCuaReadiness,
  rowButtons,
  waitForScreen,
}) {
  const rawHost = process.env.STADO_APPLE_PREPARATION_HOST;
  assert.ok(rawHost, 'STADO_APPLE_PREPARATION_HOST must explicitly name the dedicated Stado host');
  const host = rawHost.trim();
  assert.equal(host, rawHost, 'STADO_APPLE_PREPARATION_HOST must not contain surrounding whitespace');
  assert.ok(host.length > 0, 'STADO_APPLE_PREPARATION_HOST must not be empty');
  assert.doesNotMatch(host, /[\r\n\0]/u, 'STADO_APPLE_PREPARATION_HOST contains a control character');
  assert.equal(
    typeof readPromptFreeCuaReadiness,
    'function',
    'the desktop:cua runner must inject readPromptFreeCuaReadiness; it must only call check_permissions with prompt:false on the already-serving daemon',
  );

  // This is deliberately the first operation. A missing daemon or grant blocks
  // the journey before Stado Desktop can be launched and before any product
  // action can run.
  const cuaBefore = readPromptFreeCuaReadiness();
  assert.equal(
    cuaBefore?.accessibility,
    true,
    'the existing CuaDriver daemon must report Accessibility ready without prompting before app launch',
  );

  const evidence = createEvidence('stado-apple-challenge-preparation');
  const expectedCommand = preparationCommand(host);
  let app = null;
  let result = null;
  let failure = null;

  try {
    app = launchConsole();
    openScreen(app.pid, app.windowId, 'Hosts', {
      loaded: /AX\w*Button \(All hosts/,
      failures: ['No host inventory', 'No registered hosts'],
      refresh: 'Refresh',
      timeoutMs: GATES_MS,
    });
    const hosts = waitForScreen(
      app.pid,
      app.windowId,
      new RegExp(`AX\\w*Button \\(${escapedRegex(host)}(?:,|\\))`),
      GATES_MS,
    );

    const matchingRows = rowButtons(hosts).filter(
      (row) => row.label === host || row.label.startsWith(`${host},`),
    );
    assert.equal(
      matchingRows.length,
      1,
      `the real Hosts table must contain exactly one row for ${JSON.stringify(host)}; rows: ${
        rowButtons(hosts).map((row) => row.label).join(' | ') || 'none'
      }`,
    );
    click(app.pid, app.windowId, matchingRows[0].label);

    waitForScreen(
      app.pid,
      app.windowId,
      (tree) => tree.includes('Apple code capture') && tree.includes(expectedCommand),
      GATES_MS,
    );
    const selectedEvidence = evidence.capture(app.pid, app.windowId, 'selected-host');
    assert.equal(assertField(selectedEvidence, 'Command'), expectedCommand);
    click(app.pid, app.windowId, 'Read Apple readiness');
    waitForScreen(
      app.pid,
      app.windowId,
      (tree) => (
        tree.includes(`Apple code capture status read on ${host}`)
        || /AX\w*Button \(Dismiss\)/.test(tree)
      ),
      PREPARATION_MS,
    );
    const readiness = evidence.capture(app.pid, app.windowId, 'readiness-report');
    assert.ok(
      readiness.tree.includes(`Apple code capture status read on ${host}`),
      `Stado Desktop refused the read-only readiness request:\n${exactRefusal(readiness)}`,
    );
    assert.equal(assertField(readiness, 'Reported host'), host);


    click(app.pid, app.windowId, 'Prepare Apple code capture');
    waitForScreen(
      app.pid,
      app.windowId,
      (tree) => (
        tree.includes(`Apple code capture is ready on ${host}`)
        || tree.includes('Apple code capture is unavailable')
        || (/AX\w*Button \(Dismiss\)/.test(tree)
          && !tree.includes(`Apple code capture status read on ${host}`))
      ),
      PREPARATION_MS,
    );
    const observed = evidence.capture(app.pid, app.windowId, 'preparation-report');

    const refusal = !observed.tree.includes(`Apple code capture is ready on ${host}`);
    if (refusal) {
      throw new Error(
        `Stado Desktop refused Apple challenge preparation for ${JSON.stringify(host)}; exact visible refusal:\n${exactRefusal(observed)}`,
      );
    }

    assert.equal(assertField(observed, 'Reported host'), host);
    assert.ok(
      assertField(observed, 'Host-control destination').length > 0,
      'the preparation report omitted its real host-control destination',
    );
    assert.match(
      observed.tree,
      reportItemPattern('apple-challenge-helper-version', APPLE_HELPER_VERSION),
      `the product did not report Apple helper version ${APPLE_HELPER_VERSION}`,
    );
    assert.match(
      observed.tree,
      reportItemPattern('apple-challenge-accessibility', 'granted'),
      'the product did not read back the Apple helper Accessibility grant',
    );
    assert.match(
      observed.tree,
      reportItemPattern('apple-challenge-ready', 'yes'),
      'the product did not exercise the signed helper prompt-free in the registry-bound Aqua session',
    );
    assert.match(
      observed.tree,
      /apple-challenge-helper:\s*(?:installed|reused)(?=[\s"),]|$)/i,
      'the product did not report whether the real signed helper was installed or reused',
    );


    result = {
      host,
      outcome: 'ready',
      helperVersion: APPLE_HELPER_VERSION,
      readiness: 'apple-challenge-ready=yes',
    };
  } catch (error) {
    failure = error instanceof Error ? error : new Error(String(error));
  }

  try {
    const cuaAfter = readPromptFreeCuaReadiness();
    assert.deepEqual(
      cuaAfter,
      cuaBefore,
      'CuaDriver readiness changed while the Apple-only product operation ran',
    );
  } catch (error) {
    failure = addFailure(failure, error, 'checking unchanged CuaDriver readiness');
  }

  if (app) {
    try {
      dumpWindows(app.pid, 'stado-apple-challenge-preparation');
    } catch (error) {
      failure = addFailure(failure, error, 'writing the final native accessibility tree');
    }
  }
  try {
    evidence.write();
  } catch (error) {
    failure = addFailure(failure, error, 'registering native screenshots');
  }
  if (app) {
    try {
      quitApp(app.pid);
    } catch (error) {
      failure = addFailure(failure, error, 'quitting Stado Desktop');
    }
  }

  if (failure) throw failure;
  return result;
}
