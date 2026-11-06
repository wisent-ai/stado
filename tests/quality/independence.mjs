import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of the product-independence step of `stado quality
// check`: a revision whose added lines reach another catalog product the
// catalog does not show the product running without is refused, naming the
// line; the same reach already on the published main, or declared with an
// alternative whose code the workspace holds, is not.
//
// One isolated workspace under this repository's `.build`: a catalog of three
// fictional products (alpha, beta, gamma) at the path Stado reads its catalog
// from, and one canonical checkout of alpha with a bare origin whose main
// already starts `beta` from a shell script. Each case commits on top of that
// main and hands the commit to `stado quality check --root <alpha> --pre-push
// --json` as Git's pre-push hook does (a branch other than main), so only the
// committed source is checked, with no gate and no build:
//
// 1. an added line starting `gamma`, which the catalog does not declare, is
//    refused naming gamma, the file and line, and the registry command that
//    declares it; the base's `beta` line is not named;
// 2. the same line added under tests/ is not alpha running, and passes;
// 3. gamma declared with an alternative but no alternative_source is refused
//    as unproven;
// 4. gamma declared with an alternative_source the workspace holds passes;
// 5. a product the catalog does not record is not judged, and the check says
//    so naming the catalog, instead of refusing every uncatalogued checkout;
// 6. a catalog checkout whose head is behind its published main is read at
//    origin/main, where gamma is declared with its code, so the revision
//    passes; with main back at that head the same revision is refused, the
//    refusal naming the catalog, the commit and `its committed head`.
//
// Required: STADO_BIN, the Stado under test (a path, or a name on PATH).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN is required: it names the Stado whose quality check this test qualifies');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'quality-independence-'));
const workspace = join(output, 'workspace');
const checkout = join(workspace, 'alpha');
const origin = join(output, 'origin', 'alpha.git');
const catalogPath = join(workspace, 'stado', 'catalog', 'products.yml');
const report = { started_at: new Date().toISOString(), workspace, checkout, commands: [], cases: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const environment = { ...process.env, WISENT_WORKSPACE: workspace };
const MANIFEST = '.wisent-release.json';
function run(program, args, cwd = root, input) {
  const answer = spawnSync(program, args, { cwd, env: environment, encoding: 'utf8', input });
  report.commands.push({ program, arguments: args, cwd, input, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
const succeeded = answer => !answer.error && !answer.signal && !answer.status;
function success(answer) {
  assert.ok(succeeded(answer), answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
const git = args => success(run('git', ['-c', 'user.name=Stado test', '-c', 'user.email=stado-test@wisent.invalid',
  '-c', 'core.hooksPath=/dev/null', ...args], checkout));
function record(id, dependencies) {
  return {
    id, name: id, status: 'preview', visibility: 'private', family: 'wisent',
    owner_repository: `example-org/${id}`, description: `The ${id} test product.`,
    evidence: [`example-org/${id}/README.md`], roadmap: [], integrations: [],
    surfaces: [{ kind: 'cli', repository: `example-org/${id}` }],
    ...(dependencies ? { dependencies } : {}),
  };
}
// JSON is YAML, so the catalog is written as JSON.
function catalog(alphaDependencies, ids = ['alpha', 'beta', 'gamma']) {
  const beta = { product: 'beta', surface: 'cli', for_surface: 'cli',
    alternative: 'alpha runs without beta', alternative_source: 'alpha/bin/without-beta.sh' };
  writeFileSync(catalogPath, JSON.stringify({
    authority: { owner: 'stado', rule: 'test catalog' },
    products: ids.map(id => id === 'alpha' ? record(id, [beta, ...alphaDependencies]) : record(id)),
  }));
}
function commitOnMain(file, text, message) {
  git(['checkout', '--quiet', '--detach', 'origin/main']);
  mkdirSync(dirname(join(checkout, file)), { recursive: true });
  writeFileSync(join(checkout, file), text);
  git(['add', '.']);
  git(['commit', '--quiet', '-m', message]);
  return git(['rev-parse', 'HEAD']);
}
// Git's pre-push stdin: local-ref local-oid remote-ref remote-oid, the remote
// object all zeros for a branch the remote does not hold yet
// (https://git-scm.com/docs/githooks#_pre_push).
const ABSENT = '0'.repeat(createHash('sha1').digest('hex').length);
function check(name, commit) {
  const branch = `refs/heads/${name}`;
  const answer = run(stado, ['quality', 'check', '--root', checkout, '--pre-push', '--json'], root,
    `${branch} ${commit} ${branch} ${ABSENT}\n`);
  report.cases.push({ name, commit, exit_status: answer.status, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
const refusal = answer => `${answer.stdout}\n${answer.stderr}`;
let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };

  mkdirSync(dirname(catalogPath), { recursive: true });
  mkdirSync(join(checkout, 'bin'), { recursive: true });
  const own = JSON.parse(readFileSync(join(root, MANIFEST), 'utf8'));
  const platforms = Object.fromEntries(Object.keys(own.platforms).map(platform => [platform, {
    runner_platform: own.platforms[platform].runner_platform,
    quality: [{ name: 'style', kind: 'format', argv: ['sh', '-c', 'exit 4'] }],
    build: { argv: ['cat', 'VERSION'] },
    stage: { VERSION: 'VERSION' },
  }]));
  writeFileSync(join(checkout, MANIFEST), `${JSON.stringify({
    schema_version: own.schema_version, product: 'alpha', releases: true,
    version_source: { kind: 'text', path: 'VERSION' }, platforms,
    promotion: { channels: ['candidate'], reconcile: false },
  }, null, '  ')}\n`);
  writeFileSync(join(checkout, 'VERSION'), '0.1.0\n');
  // The base already starts beta, declared with code that runs without it.
  writeFileSync(join(checkout, 'bin', 'run.sh'), '#!/bin/sh\nbeta --version\n');
  writeFileSync(join(checkout, 'bin', 'without-beta.sh'), '#!/bin/sh\necho alpha alone\n');
  writeFileSync(join(checkout, 'bin', 'without-gamma.sh'), '#!/bin/sh\necho alpha without gamma\n');
  success(run('git', ['init', '--quiet', '--bare', '--initial-branch=main', origin]));
  git(['init', '--quiet', '--initial-branch=main']);
  // A GitHub origin, as a canonical checkout has, served by the local bare
  // repository through this checkout's own configuration.
  const github = 'https://github.com/example-org/alpha.git';
  git(['config', `url.file://${origin}.insteadOf`, github]);
  git(['remote', 'add', 'origin', github]);
  git(['add', '.']);
  git(['commit', '--quiet', '-m', 'alpha starts beta']);
  git(['push', '--quiet', 'origin', 'HEAD:main']);
  git(['fetch', '--quiet', 'origin']);

  // 1. An added line starting gamma, undeclared: refused naming the line.
  catalog([]);
  const undeclared = commitOnMain('bin/gamma.sh', '#!/bin/sh\ngamma --help\n', 'alpha starts gamma');
  let answer = check('undeclared', undeclared);
  assert.ok(!succeeded(answer), `an undeclared added reach passed: ${answer.stderr}`);
  assert.match(refusal(answer), /alpha cannot be shown to run without gamma: the catalog declares no dependency of alpha on gamma/);
  assert.match(refusal(answer), /added at bin\/gamma\.sh:2/);
  assert.match(refusal(answer), /stado product registry set alpha --add-dependency/);
  assert.doesNotMatch(refusal(answer), /without beta:|bin\/run\.sh/, 'the base line already on main was judged');

  // 2. The same call added in the product's tests is not alpha running.
  const tested = commitOnMain('tests/gamma.sh', '#!/bin/sh\ngamma --help\n', 'a test starts gamma');
  answer = check('tests', tested);
  assert.ok(succeeded(answer), `a call under tests/ was judged as alpha running: ${answer.stderr}`);

  // 3. Declared with an alternative and no code named for it: unproven.
  catalog([{ product: 'gamma', surface: 'cli', for_surface: 'cli', alternative: 'alpha runs without gamma' }]);
  answer = check('unproven', undeclared);
  assert.ok(!succeeded(answer), `an alternative with no code passed: ${answer.stderr}`);
  assert.match(refusal(answer), /names no alternative_source, so no code shows alpha running without gamma/);

  // 4. Declared with code the workspace holds: alpha runs without gamma.
  catalog([{ product: 'gamma', surface: 'cli', for_surface: 'cli', alternative: 'alpha runs without gamma',
    alternative_source: 'alpha/bin/without-gamma.sh' }]);
  answer = check('independent', undeclared);
  assert.ok(succeeded(answer), `a declared, proven reach was refused: ${answer.stderr}`);

  // 5. A product the catalog does not record has nothing to judge against:
  //    the check passes and says why the lines were not judged.
  catalog([], ['beta', 'gamma']);
  answer = check('unrecorded', undeclared);
  assert.ok(succeeded(answer), `an uncatalogued product was refused: ${answer.stderr}`);
  assert.match(answer.stderr, /not judged for product independence: the catalog .*products\.yml as it stands names no product alpha/);

  // 6. The catalog in a checkout whose head is behind its published main is
  //    read at origin/main, as every catalog reader reads it: main declares
  //    gamma with its code, the checkout's head does not, and the check
  //    passes saying nothing more; the head alone refuses, naming it.
  const stadoCheckout = dirname(dirname(catalogPath));
  const stadoOrigin = join(output, 'origin', 'stado.git');
  const inStado = args => success(run('git', ['-c', 'user.name=Stado test', '-c', 'user.email=stado-test@wisent.invalid',
    '-c', 'core.hooksPath=/dev/null', ...args], stadoCheckout));
  success(run('git', ['init', '--quiet', '--bare', '--initial-branch=main', stadoOrigin]));
  inStado(['init', '--quiet', '--initial-branch=main']);
  inStado(['remote', 'add', 'origin', stadoOrigin]);
  catalog([]);
  inStado(['add', '.']);
  inStado(['commit', '--quiet', '-m', 'catalog without gamma']);
  const lagging = inStado(['rev-parse', 'HEAD']);
  catalog([{ product: 'gamma', surface: 'cli', for_surface: 'cli', alternative: 'alpha runs without gamma',
    alternative_source: 'alpha/bin/without-gamma.sh' }]);
  inStado(['commit', '--quiet', '-am', 'alpha declares gamma']);
  inStado(['push', '--quiet', 'origin', 'HEAD:main']);
  inStado(['reset', '--quiet', '--hard', lagging]);
  inStado(['fetch', '--quiet', 'origin']);
  answer = check('lagging', undeclared);
  assert.ok(succeeded(answer), `a reach the published catalog declares was refused for a lagging checkout: ${answer.stderr}`);
  success(run('git', ['--git-dir', stadoOrigin, 'update-ref', 'refs/heads/main', lagging]));
  inStado(['fetch', '--quiet', '--force', 'origin', 'main:refs/remotes/origin/main']);
  answer = check('head', undeclared);
  assert.ok(!succeeded(answer), 'an undeclared reach passed against the committed head');
  assert.match(refusal(answer), new RegExp(`judged against the catalog .*catalog/products\\.yml at ${lagging} \\(its committed head\\)`));
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'One isolated canonical checkout with a bare origin and a three-product catalog; stado quality check --pre-push --json on five committed revisions (undeclared, under tests/, unproven, declared with code, an unrecorded product) and on a catalog checkout behind and level with its published main; GUI parity is the Desktop operations quality-revision (the same quality check) and dependencies';
  rmSync(workspace, { recursive: true, force: true });
  rmSync(dirname(origin), { recursive: true, force: true });
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, '  ')}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
