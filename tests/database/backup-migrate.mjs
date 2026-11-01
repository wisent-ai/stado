import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real journey of `stado database backup` and `stado database migrate`
// against a dedicated, already provisioned profile: its home, config, owner
// vault and keyring live inside this checkout's ignored .build, and its
// declared source database is an external Postgres server provisioned for
// this test with rows in it. The journey backs it up, checks the manifest
// against an independent count, lists it with `backups`, takes a second
// backup with `--keep` and checks which backups stay, moves the database onto
// a fleet Postgres on this host, counts again there, reads the items, serves
// the REST API beside it (reads a table through it, reuses the placement,
// rotates the keys and checks the old one is refused), serves sign-in beside
// it (a user signs in, the token opens the REST API, the refresh token is
// taken, a wrong password is refused, the placement is reused and retired),
// retires the API, checks the refusals, and destroys the fleet copy.
// Required: STADO_DATABASE_TEST_HOME, STADO_DATABASE_TEST_CONFIG,
// STADO_DATABASE_TEST_KEYRING, STADO_DATABASE_TEST_NAME (the declared external
// source), STADO_DATABASE_TEST_PORT (the port the fleet copy listens on),
// STADO_DATABASE_TEST_KEEP (the retention the second backup states),
// STADO_DATABASE_TEST_REST_PORT and STADO_DATABASE_TEST_KEY_DAYS (the port the
// fleet REST API listens on and its key lifetime; `postgrest` on this host),
// STADO_DATABASE_TEST_AUTH_PORT, STADO_DATABASE_TEST_AUTH_READY_SECONDS and
// STADO_DATABASE_TEST_AUTH_PROJECT (the sign-in server's port, the seconds it
// is given to answer, and a dedicated test Supabase project whose sign-in
// settings it takes), STADO_BIN and SKARBIEC_BIN (the builds under test). No
// production defaults. A failed journey writes its report and then throws.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const build = realpathSync(join(root, '.build'));
function fixturePath(variable) {
  assert.ok(process.env[variable], `${variable} must name a dedicated test fixture`);
  const path = realpathSync(process.env[variable]);
  const within = relative(build, path);
  assert.ok(within && !isAbsolute(within) && !within.startsWith('..'), `${variable} must be inside ${build}`);
  return path;
}
function required(variable) {
  assert.ok(process.env[variable], `${variable} must be stated for this journey`);
  return process.env[variable];
}
const home = fixturePath('STADO_DATABASE_TEST_HOME');
const config = fixturePath('STADO_DATABASE_TEST_CONFIG');
const keyring = fixturePath('STADO_DATABASE_TEST_KEYRING');
const name = required('STADO_DATABASE_TEST_NAME');
const port = required('STADO_DATABASE_TEST_PORT');
const stado = resolve(required('STADO_BIN'));
const skarbiec = resolve(required('SKARBIEC_BIN'));
const configuration = () => JSON.parse(readFileSync(config, 'utf8'));
const declaration = configuration().database_api?.databases?.[name];
assert.equal(declaration?.engine, 'postgres', 'The source must be a declared postgres database');
const item = `${name}-database`;
const vault = realpathSync(configuration().secrets.skarbiec.vault_file);
assert.ok(!relative(home, vault).startsWith('..') && !isAbsolute(relative(home, vault)),
  'The owner vault must belong to the dedicated test home');
const output = mkdtempSync(join(build, 'database-backup-migrate-'));
const temporary = join(output, 'temporary');
mkdirSync(temporary, { mode: constants.S_IRWXU });
const environment = {
  PATH: process.env.PATH, HOME: home, STADO_CONFIG: config, GNUPGHOME: keyring, TMPDIR: temporary,
  SKARBIEC_VAULT_FILE: vault, SKARBIEC_AUDIT_FILE: join(output, 'audit.jsonl'), SKARBIEC_BIN: skarbiec,
};
const report = { started_at: new Date().toISOString(), commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args, extra = {}) {
  const answer = spawnSync(program, args, { cwd: root, env: { ...environment, ...extra }, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.equal(answer.status, constants.F_OK,
    `exit ${answer.status}, signal ${answer.signal}, error ${answer.error?.message}: ${answer.stderr}`);
  return answer.stdout.trim();
}
function refused(answer, words) {
  assert.notEqual(answer.status, constants.F_OK, `expected a refusal naming ${words}`);
  assert.ok(answer.stderr.includes(words), `the refusal must say ${words}: ${answer.stderr}`);
}
function field(id, key) {
  return success(run(skarbiec, ['get', id, '--field', key]));
}
// Every table's exact count, read with psql directly from the item, not
// through Stado: the independent witness the manifest is held against.
function counted(id) {
  const url = new URL(field(id, 'session_url'));
  url.password = '';
  const ca = join(temporary, `${id}-ca.pem`);
  writeFileSync(ca, field(id, 'ca_certificate'));
  const sql = "select format('%I.%I', table_schema, table_name), (xpath('/row/c/text()', query_to_xml(format('select count(*) as c from %I.%I', table_schema, table_name), false, true, '')))[1]::text from information_schema.tables where table_type = 'BASE TABLE' and table_schema !~ '^pg_' and table_schema <> 'information_schema' order by table_schema, table_name";
  const text = success(run('psql', ['-X', '-A', '-t', '-F', '\t', '-c', sql, `--dbname=${url}`],
    { PGPASSWORD: field(id, 'db_password'), PGSSLMODE: 'verify-full', PGSSLROOTCERT: ca }));
  return Object.fromEntries(text.split('\n').filter(Boolean).map(line => {
    const [table, rows] = line.split('\t');
    return [table, Number(rows)];
  }));
}
let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.source_diff = success(run('git', ['diff', 'HEAD', '--', 'stado-rs/src/cli/database', 'tests/database']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.stado = { path: stado, sha256: digest(readFileSync(stado)), version: success(run(stado, ['--version'])) };
  report.skarbiec = { path: skarbiec, sha256: digest(readFileSync(skarbiec)), version: success(run(skarbiec, ['version'])) };
  assert.equal(field(item, 'provider'), 'external', 'The source must be an external server, never a hosted project');
  const before = counted(item);
  assert.ok(Object.values(before).some(Boolean), 'The source fixture must hold rows');
  report.before = before;

  refused(run(stado, ['database', 'backup', `${name}-undeclared`, '--json']), 'is not declared');
  const rows = JSON.parse(success(run(stado, ['database', 'backup', name, '--json'])));
  const kept = rows.find(row => row.database === name);
  assert.ok(kept?.backup, `backup answered no directory: ${JSON.stringify(rows)}`);
  const manifest = JSON.parse(readFileSync(join(kept.backup, 'manifest.json'), 'utf8'));
  assert.deepEqual(manifest.tables, before, 'The manifest must count exactly what the source holds');
  const archive = join(kept.backup, 'database.dump');
  assert.equal(digest(readFileSync(archive)), manifest.sha256);
  assert.ok(!(statSync(archive).mode & (constants.S_IRWXG | constants.S_IRWXO)), 'The archive must be owner-only');
  report.backup = { directory: kept.backup, manifest };
  const listed = JSON.parse(success(run(stado, ['database', 'backups', name, '--json'])));
  assert.ok(listed.some(row => row.backup === kept.backup && row.rows === manifest.rows),
    'backups must list the backup just taken with its rows');
  refused(run(stado, ['database', 'backup', name, '--keep', String(Number()), '--json']), '--keep');
  const keep = required('STADO_DATABASE_TEST_KEEP');
  const second = JSON.parse(success(run(stado, ['database', 'backup', name, '--keep', keep, '--json'])))
    .find(row => row.database === name);
  const complete = rows => rows.filter(row => !row.incomplete).map(row => row.backup);
  const expected = [second.backup, ...complete(listed)].slice(Number(), Number(keep));
  const after = complete(JSON.parse(success(run(stado, ['database', 'backups', name, '--json']))));
  assert.deepEqual(after, expected, '--keep must leave exactly the newest complete backups it names');
  report.retention = { listed, second, after };

  refused(run(stado, ['database', 'migrate', name, '--to', 'supabase', '--port', port, '--json']), 'is not a destination migrate places');
  const moved = JSON.parse(success(run(stado, ['database', 'migrate', name, '--to', 'fleet', '--port', port, '--json'])));
  assert.equal(moved.migrated, name);
  assert.deepEqual(moved.restore.mismatches, []);
  assert.equal(field(item, 'provider'), 'fleet', 'The item must point at the fleet copy');
  assert.equal(field(`${item}-previous`, 'provider'), 'external', 'The source coordinates must be kept');
  assert.deepEqual(counted(item), before, 'The fleet copy must hold every row the source held');
  assert.deepEqual(configuration().database_api.databases[name], declaration, 'The declaration must not change');
  report.migrated = moved;

  refused(run(stado, ['database', 'migrate', name, '--to', 'fleet', '--port', port, '--json']), 'is already a fleet database');
  const restPort = required('STADO_DATABASE_TEST_REST_PORT');
  const keyDays = required('STADO_DATABASE_TEST_KEY_DAYS');
  const served = JSON.parse(success(run(stado, ['database', 'rest', 'place', name, '--port', restPort, '--key-valid-days', keyDays, '--json'])));
  assert.equal(served.reused, false);
  assert.equal(served.unit, `${name}-rest`);
  const anonKey = field(item, 'anon_key');
  const serviceKey = field(item, 'service_role_key');
  assert.notEqual(anonKey, serviceKey, 'the two keys name different roles');
  // The API answers the provider's dialect: a table of the restored
  // database read with the service key, and a request without a key
  // reaches it as anon, whose policies the dump decides.
  const [table] = Object.keys(before).map(name => name.split('.').at(-1));
  const answer = await fetch(`${served.rest_url}/${table}?select=*&limit=${Object.keys(before).length}`, {
    headers: { apikey: serviceKey, Authorization: `Bearer ${serviceKey}` },
  });
  assert.ok(answer.ok, `the fleet REST API answered ${answer.status} for ${table}`);
  assert.ok(Array.isArray(await answer.json()), 'the API answers rows as a list');
  const again = JSON.parse(success(run(stado, ['database', 'rest', 'place', name, '--port', restPort, '--key-valid-days', keyDays, '--json'])));
  assert.equal(again.reused, true, 'a placed API is reused, not placed twice');
  assert.equal(field(item, 'anon_key'), anonKey, 'keys are not minted again without --rotate-keys');
  report.rest = { served, status: answer.status };
  const rotated = JSON.parse(success(run(stado, ['database', 'rest', 'place', name, '--port', restPort, '--key-valid-days', keyDays, '--rotate-keys', '--json'])));
  assert.equal(rotated.keys_rotated, true);
  const rotatedKey = field(item, 'service_role_key');
  assert.notEqual(rotatedKey, serviceKey, '--rotate-keys mints a new service key');
  const withOld = await fetch(`${served.rest_url}/${table}?select=*`, { headers: { apikey: serviceKey, Authorization: `Bearer ${serviceKey}` } });
  assert.ok(!withOld.ok, `the old key must be refused after rotation, got ${withOld.status}`);
  const withNew = await fetch(`${served.rest_url}/${table}?select=*`, { headers: { apikey: rotatedKey, Authorization: `Bearer ${rotatedKey}` } });
  assert.ok(withNew.ok, `the rotated key must answer, got ${withNew.status}`);
  report.rest.rotated = { rotated, oldKeyStatus: withOld.status, newKeyStatus: withNew.status };
  // Sign-in beside the fleet copy, with the settings of a dedicated test
  // Supabase project: the server answers /health, a user created through
  // its admin API signs in with a password, the token it issues opens the
  // REST API signed with the same secret, its refresh token is taken, a
  // wrong password is refused, a second placement is reused, and once the
  // REST API's secret is gone the placement is refused.
  const authPort = required('STADO_DATABASE_TEST_AUTH_PORT');
  const authReady = required('STADO_DATABASE_TEST_AUTH_READY_SECONDS');
  const authProject = required('STADO_DATABASE_TEST_AUTH_PROJECT');
  const placeAuth = () => run(stado, ['database', 'auth', 'place', name, '--port', authPort,
    '--ready-within', authReady, '--from-project', authProject, '--json']);
  const signIn = JSON.parse(success(placeAuth()));
  assert.equal(signIn.reused, false);
  assert.equal(signIn.unit, `${name}-auth`);
  assert.equal(field(item, 'auth_url'), signIn.auth_url, 'the item carries the sign-in address');
  const health = await fetch(`${signIn.auth_url}/health`);
  assert.ok(health.ok, `the sign-in server answered ${health.status} for /health`);
  const anon = field(item, 'anon_key');
  const email = `journey-${Date.now()}@stado.invalid`;
  const password = createHash('sha256').update(email).digest('hex');
  const created = await fetch(`${signIn.auth_url}/admin/users`, {
    method: 'POST',
    headers: { apikey: rotatedKey, Authorization: `Bearer ${rotatedKey}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ email, password, email_confirm: true }),
  });
  assert.ok(created.ok, `the admin API refused the journey's user: ${created.status} ${await created.text()}`);
  const token = await fetch(`${signIn.auth_url}/token?grant_type=password`, {
    method: 'POST',
    headers: { apikey: anon, 'Content-Type': 'application/json' },
    body: JSON.stringify({ email, password }),
  });
  assert.ok(token.ok, `the journey's user could not sign in: ${token.status} ${await token.clone().text()}`);
  const session = await token.json();
  assert.ok(session.access_token && session.refresh_token, 'a sign-in answers an access and a refresh token');
  const asUser = await fetch(`${served.rest_url}/${table}?select=*&limit=1`, {
    headers: { apikey: anon, Authorization: `Bearer ${session.access_token}` },
  });
  assert.notEqual(asUser.status, 401, 'the REST API takes a token the sign-in server issued');
  const refreshed = await fetch(`${signIn.auth_url}/token?grant_type=refresh_token`, {
    method: 'POST',
    headers: { apikey: anon, 'Content-Type': 'application/json' },
    body: JSON.stringify({ refresh_token: session.refresh_token }),
  });
  assert.ok(refreshed.ok, `a refresh token was refused: ${refreshed.status}`);
  const wrong = await fetch(`${signIn.auth_url}/token?grant_type=password`, {
    method: 'POST',
    headers: { apikey: anon, 'Content-Type': 'application/json' },
    body: JSON.stringify({ email, password: `${password}-wrong` }),
  });
  assert.ok(!wrong.ok, `a wrong password must be refused, got ${wrong.status}`);
  const signInAgain = JSON.parse(success(placeAuth()));
  assert.equal(signInAgain.reused, true, 'a placed sign-in server is reused, not placed twice');
  const authRetired = JSON.parse(success(run(stado, ['database', 'auth', 'retire', name, '--json'])));
  assert.equal(authRetired.unit_status, 'removed');
  assert.ok(authRetired.fields_removed.includes('auth_url'), 'retire takes auth_url off the item');
  refused(run(stado, ['database', 'auth', 'retire', name, '--json']), 'has no sign-in server to retire');
  report.auth = { signIn, health: health.status, signedIn: token.status, asUser: asUser.status,
    refreshed: refreshed.status, wrongPassword: wrong.status, retired: authRetired };
  const retired = JSON.parse(success(run(stado, ['database', 'rest', 'retire', name, '--json'])));
  assert.equal(retired.unit_status, 'removed');
  assert.ok(retired.fields_removed.includes('anon_key'), 'retire takes the keys off the item');
  refused(run(stado, ['database', 'rest', 'retire', name, '--json']), 'has no REST API to retire');
  report.rest.retired = retired;
  refused(placeAuth(), 'holds no jwt_secret');
  const destroyed = JSON.parse(success(run(stado, ['database', 'destroy', name, '--json'])));
  assert.equal(destroyed.provider, 'fleet');
  report.destroyed = destroyed;
  report.result = 'passed';
} catch (error) {
  failure = error;
  report.error = error.stack;
} finally {
  const stopped = run('gpgconf', ['--homedir', keyring, '--kill', 'all']);
  if (stopped.status !== constants.F_OK) {
    report.result = 'failed';
    report.cleanup_error = 'The dedicated test keyring helpers were not confirmed stopped';
    if (!failure) failure = new Error(report.cleanup_error);
  } else {
    rmSync(temporary, { recursive: true, force: true });
  }
  report.finished_at = new Date().toISOString();
  report.scope = 'Real backup of an external Postgres fixture, its move onto a fleet Postgres on this host with row counts held against psql, the kept items, refusals and the fleet copy destroyed; not a hosted-provider source';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
