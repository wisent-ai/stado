import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import {
  closeSync,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  realpathSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const FIXTURE_TEST = 'service_convergence_cua_fixture';

async function waitForFile(file, child, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (!existsSync(file)) {
    assert.equal(
      child.exitCode,
      null,
      `the real convergence fixture exited before readiness with ${child.exitCode}`,
    );
    assert.equal(
      child.signalCode,
      null,
      `the real convergence fixture exited before readiness with ${child.signalCode}`,
    );
    assert.ok(Date.now() < deadline, `the real convergence fixture wrote no readiness file at ${file}`);
    await delay(Number('100'));
  }
}

async function waitForExit(child, timeoutMs) {
  const exitWithin = (limitMs) => {
    if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve(true);
    return new Promise((resolveExit) => {
      const onExit = () => {
        clearTimeout(timer);
        resolveExit(true);
      };
      const timer = setTimeout(() => {
        child.off('exit', onExit);
        resolveExit(false);
      }, limitMs);
      child.once('exit', onExit);
    });
  };
  if (!(await exitWithin(timeoutMs))) {
    child.kill('SIGTERM');
    await exitWithin(Number('5000'));
    throw new Error('the real convergence fixture did not stop after the CUA journey');
  }
  assert.equal(
    child.exitCode,
    0,
    `the real convergence fixture exited with ${child.exitCode ?? child.signalCode}`,
  );
}

export async function runServicesConvergenceJourney({
  activate,
  assertAbsent,
  click,
  createEvidence,
  dumpWindows,
  launchConsole,
  quitApp,
  readWindow,
  sleep,
  waitForScreen,
}) {
  const source = process.env.PROBIERZ_APP_SOURCE;
  const builtCli = process.env.PROBIERZ_STADO_BIN;
  assert.ok(source, 'PROBIERZ_APP_SOURCE must identify the staged Stado source');
  assert.ok(builtCli, 'PROBIERZ_STADO_BIN must identify the CLI built from that staged source');
  assert.ok(existsSync(builtCli), `the staged-source Stado CLI is absent: ${builtCli}`);

  const crate = path.resolve(source, 'stado-rs');
  assert.ok(process.env.PROBIERZ_ARTIFACTS, 'PROBIERZ_ARTIFACTS is required');
  const artifacts = path.resolve(process.env.PROBIERZ_ARTIFACTS);
  const control = path.join(artifacts, 'stado-service-convergence-fixture');
  mkdirSync(control, { recursive: true });
  const ready = path.join(control, 'ready.json');
  const stop = path.join(control, 'stop');
  const stdoutPath = path.join(control, 'fixture.stdout.log');
  const stderrPath = path.join(control, 'fixture.stderr.log');
  const stdout = openSync(stdoutPath, 'w', 0o600);
  const stderr = openSync(stderrPath, 'w', 0o600);
  const fixture = spawn('cargo', [
    'test', '--locked', '--test', 'service_convergence', FIXTURE_TEST,
    '--', '--ignored', '--exact', '--nocapture', '--test-threads=1',
  ], {
    cwd: crate,
    env: {
      ...process.env,
      STADO_SERVICE_CONVERGENCE_READY: ready,
      STADO_SERVICE_CONVERGENCE_STOP: stop,
      CARGO_PROFILE_TEST_DEBUG: '0',
      CARGO_INCREMENTAL: '0',
    },
    stdio: ['ignore', stdout, stderr],
  });
  closeSync(stdout);
  closeSync(stderr);

  const evidence = createEvidence('stado-services-convergence');
  let app = null;
  try {
    await waitForFile(ready, fixture, Number('180000'));
    const state = JSON.parse(readFileSync(ready, 'utf8'));
    for (const field of ['endpoint', 'home', 'storage', 'config', 'binary', 'target', 'token_file']) {
      assert.ok(state[field], `the real fixture readiness document has no ${field}`);
    }
    assert.equal(
      realpathSync(state.binary),
      realpathSync(builtCli),
      'the GUI fixture and Stado Desktop are not using the same staged-source CLI',
    );
    assert.equal(
      statSync(state.token_file).mode & 0o777,
      0o600,
      'the dedicated registry API token file is not owner read/write only',
    );
    assert.equal(
      existsSync(path.join(state.home, '.stado/local-storage/registry.json')),
      false,
      'the isolated app HOME unexpectedly exposes the fixture target to a CLI fallback',
    );

    app = launchConsole({
      env: {
        HOME: state.home,
        CFFIXED_USER_HOME: state.home,
        TMPDIR: path.join(state.home, 'tmp'),
        PATH: `${path.dirname(builtCli)}:/usr/bin:/bin:/usr/sbin:/sbin`,
        STADO_REGISTRY_API_URL: state.endpoint,
        STADO_REGISTRY_API_TOKEN_FILE: state.token_file,
      },
      args: ['-dashboardBaseURL', state.endpoint],
    });

    const loaded = openServices(app, state.target, {
      click,
      readWindow,
      sleep,
      waitForScreen,
    });
    assertAbsent(
      loaded,
      ['Connect to Stado', 'This source cannot be read', 'Sign In', 'Continue with', 'Enter your email'],
      'the documented local control path selected an unreadable source or requested an account operation',
    );
    assert.match(loaded.tree, /skarbiec/i, 'the real host-wide GET report is absent');
    evidence.capture(app.pid, app.windowId, 'host-wide-report-before-apply');

    const sheet = activate(app.pid, app.windowId, 'Converge…', {
      needle: 'Converge declared service binaries',
      timeoutMs: Number('30000'),
    });
    assert.match(sheet.view.tree, new RegExp(state.target));
    assert.match(sheet.view.tree, /All declared binaries/);
    assert.match(sheet.view.tree, /stado service converge/);
    evidence.capture(app.pid, sheet.windowId, 'local-registry-client-host-wide-confirmation');
    click(app.pid, sheet.windowId, 'Apply convergence');

    const receipt = waitForScreen(
      app.pid,
      app.windowId,
      (tree) => tree.includes('Convergence receipt')
        && /exit [1-9][0-9]*/.test(tree)
        && tree.includes('skarbiec')
        && tree.includes('"status": "failed"'),
      Number('180000'),
    );
    assert.match(receipt.tree, new RegExp(`"target": "${state.target}"`));
    assert.match(receipt.tree, /"verdict": "host-behind"/);
    assert.match(receipt.tree, /"verdict": "in-sync"/);
    evidence.capture(app.pid, app.windowId, 'complete-failed-receipt');

    click(app.pid, app.windowId, 'Refresh');
    const retained = waitForScreen(
      app.pid,
      app.windowId,
      (tree) => tree.includes('Convergence receipt')
        && /exit [1-9][0-9]*/.test(tree)
        && tree.includes('skarbiec')
        && tree.includes('"status": "failed"'),
      Number('180000'),
    );
    assert.match(
      retained.tree,
      /Refreshing service state does not replace them/,
      'refresh discarded the completed failed convergence receipt',
    );
    evidence.capture(app.pid, app.windowId, 'failed-receipt-retained-after-refresh');
    evidence.write();
  } catch (error) {
    if (app) dumpWindows(app.pid, 'stado-services-convergence-failure');
    throw error;
  } finally {
    if (app) quitApp(app.pid);
    writeFileSync(stop, 'stop\n', { mode: 0o600 });
    await waitForExit(fixture, Number('30000'));
  }
}

function openServices(app, target, { click, readWindow, sleep }) {
  click(app.pid, app.windowId, 'Services');
  const deadline = Date.now() + Number('180000');
  for (;;) {
    const view = readWindow(app.pid, app.windowId);
    if (view.tree.includes(target) && /skarbiec/i.test(view.tree)) return view;
    if (/Sign In|Continue with|Enter your email|Connect to Stado/.test(view.tree)) {
      throw new Error(
        'The dedicated local registry API client unexpectedly requested an account sign-in; this journey performs no Wisent account operation or provider flow.',
      );
    }
    assert.ok(
      Date.now() < deadline,
      `Services never rendered the real host-wide convergence report; last tree: ${view.tree.slice(-2500)}`,
    );
    if (view.tree.includes('Retry')) click(app.pid, app.windowId, 'Retry');
    sleep(Number('500'));
  }
}
