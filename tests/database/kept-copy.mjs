import assert from 'node:assert/strict';

// Skarbiec refuses a second item holding exactly another's payload, so the
// copy `stado database migrate` keeps of an item says what it is
// (`context.kept_as {role, of, at}`) and the item in use does not.
// `read(id)` answers the item's payload as `skarbiec get` prints it.
export function assertKeptCopy(read, item, copy, role) {
  const keptAs = read(copy).context?.kept_as;
  assert.equal(keptAs?.role, role, `${copy} must say it is the kept ${role} copy: ${JSON.stringify(keptAs)}`);
  assert.equal(keptAs?.of, item, `${copy} must name the item it stands for`);
  assert.ok(!Number.isNaN(Date.parse(keptAs?.at)), `${copy} must say when it was kept: ${keptAs?.at}`);
  assert.equal(read(item).context?.kept_as, undefined, `${item} in use must not carry kept_as`);
}
