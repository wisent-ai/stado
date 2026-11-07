import assert from 'node:assert/strict';

// The network-name leg of tests/database/backup-migrate.mjs, run on its
// dedicated profile once the database stands on the fleet server: a
// placement made while the host's first SSH destination was its Stado
// network name (`<host>.stado.internal`) recorded that name as its address,
// where no resolver answers it, so psql, pg_restore and every consumer's
// driver failed with `could not translate host name`.
// - the item is put back on the network name (address, pooler_url,
//   session_url), as such a placement wrote it, and a count through it is
//   refused naming the name the resolver could not translate;
// - `stado database tls reissue` re-addresses it to the address the
//   placement wrote, the URLs name that address again, the unit is restarted
//   and the certificate is issued for it, its answer carries the fields a
//   plain reissue answers (the desktop reads them) and names no REST API or
//   sign-in server left at the old name (none is placed yet), and the count
//   through the item holds every row again.
// `ops` carries the journey's own helpers: run, success, field, put,
// payloadOf, counted, before (the counts the fleet copy must hold), stado,
// skarbiec, item, name and here.
export function networkLeg(ops) {
  const { run, success, field, put, payloadOf, counted, before, stado, skarbiec, item, name, here } = ops;
  const placed = payloadOf(JSON.parse(success(run(skarbiec, ['get', item]))));
  const address = placed.fields.address;
  const networked = `${here}.stado.internal`;
  const renamed = url => {
    const parsed = new URL(url);
    parsed.hostname = networked;
    return parsed.toString();
  };
  put(item, { ...placed, fields: { ...placed.fields, address: networked,
    pooler_url: renamed(placed.fields.pooler_url), session_url: renamed(placed.fields.session_url) } });
  assert.throws(() => counted(item), new RegExp(networked.replaceAll('.', '\\.')),
    'a session at a Stado network name must fail naming the name no resolver answers');
  const reissued = JSON.parse(success(run(stado, ['database', 'tls', 'reissue', name, '--json'])));
  const readdressed = reissued.readdressed;
  assert.equal(readdressed?.from, networked, `the reissue must name the network address it replaced: ${JSON.stringify(reissued)}`);
  assert.equal(readdressed?.to, address, 'the item must name the address the placement wrote');
  assert.equal(readdressed?.unit_status, 'restarted', 'a managed unit is restarted on the new certificate');
  assert.equal(reissued.address, address, 'the answer names the address its certificate is issued for');
  assert.equal(reissued.certificate, readdressed?.certificate, 'the answer names the certificate written');
  assert.equal(reissued.unit, `${name}-database`, 'the answer names the unit serving the database');
  assert.deepEqual(readdressed?.stale, [], 'no REST API or sign-in server is placed yet, so none is named stale');
  assert.equal(field(item, 'address'), address);
  assert.equal(new URL(field(item, 'session_url')).hostname, new URL(placed.fields.session_url).hostname,
    'session_url must name the resolvable address again');
  assert.equal(new URL(field(item, 'pooler_url')).hostname, new URL(placed.fields.pooler_url).hostname,
    'pooler_url must name the resolvable address again');
  assert.deepEqual(counted(item), before, 'the re-addressed server must hold every row');
  const again = JSON.parse(success(run(stado, ['database', 'tls', 'reissue', name, '--json'])));
  assert.equal(again.readdressed, undefined, 'an item at a resolvable address is only reissued, never re-addressed');
  return reissued;
}
