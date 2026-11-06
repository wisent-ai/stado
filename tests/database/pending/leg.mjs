import assert from 'node:assert/strict';

// The pending-placement leg of tests/database/backup-migrate.mjs, run on its
// dedicated profile with the source already silent and before the move: a
// placement an earlier failed run left as `<item>-fleet-pending` pins the
// move to its host and port, and only --discard-pending lets the move place
// anew.
// - --discard-pending with none pending is refused naming the missing item;
// - with a pending item planted on this host at another port (as migrate
//   writes one after a placement that stored its item and failed to install
//   its unit), a run naming the journey's port is refused naming the pending
//   item, where it stands and the flag, leaving the item and the pending item
//   as they were;
// - --discard-pending then retires it (this host declares no unit for it, so
//   the unit is already absent; the item is deleted), places on the stated
//   port and restores. The move's report is returned for the journey's own
//   checks of the moved database.
// `ops` carries the journey's own helpers: run, success, refused, field, put
// (writes an item's payload), here (this host's registry name), source (the
// silent item's payload) and its migrate arguments.
export function pendingLeg(ops) {
  const { run, success, refused, field, put, stado, skarbiec, item, port, here, source, fromBackup, backup } = ops;
  const pending = `${item}-fleet-pending`;
  const discarding = [...fromBackup(backup).slice(0, -1), '--discard-pending', '--json'];
  refused(run(stado, discarding), `the vault holds no ${pending}: there is no pending placement to discard`);
  const pendingPort = Number(port) + 1;
  put(pending, {
    ...source,
    fields: { ...source.fields, provider: 'fleet', host: here, port: pendingPort },
    context: { ...source.context, kept_as: { role: 'fleet-pending', of: item, at: new Date().toISOString() } },
  });
  const pinned = run(stado, fromBackup(backup));
  refused(pinned, `${pending} keeps the fleet server an earlier run of migrate placed on ${here}:${pendingPort}`);
  refused(pinned, 'Add --discard-pending');
  assert.equal(field(item, 'provider'), 'external', 'A move refused for a pending placement must leave the item on the source');
  assert.equal(field(pending, 'port'), String(pendingPort), 'A refused move must keep the pending item as it was');
  const moved = JSON.parse(success(run(stado, discarding)));
  const discarded = moved.discarded_pending;
  assert.equal(discarded?.item_status, 'deleted', `The pending item must be deleted: ${JSON.stringify(discarded)}`);
  assert.equal(discarded?.unit_status, 'already absent', 'This host declared no unit for the planted placement');
  assert.equal(discarded?.host, here, 'The discarded placement is the one planted on this host');
  refused(run(skarbiec, ['get', pending]), pending);
  assert.equal(field(item, 'port'), port, 'The item must name the port this move placed on, not the discarded one');
  return moved;
}
