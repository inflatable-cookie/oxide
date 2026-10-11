// Independent Node implementation of the Rust qualification framing contract.
import { readFileSync } from 'node:fs';
import { createHmac, createHash, timingSafeEqual } from 'node:crypto';
import assert from 'node:assert/strict';
const v = JSON.parse(readFileSync(new URL('../fixtures/qualification-auth-v1.json', import.meta.url)));
const frame = Buffer.concat([v.domain, ...v.fields].flatMap(value => {
  const bytes = Buffer.from(value, 'utf8');
  const size = Buffer.alloc(4); size.writeUInt32BE(bytes.length);
  return [size, bytes];
}));
assert.equal(frame.toString('hex'), v.frame_hex);
const tag = createHmac('sha256', Buffer.from(v.key_hex, 'hex')).update(frame).digest();
assert(timingSafeEqual(tag, Buffer.from(v.receipt.slice('hmac-sha256-v1:'.length), 'hex')));
const fields = Object.fromEntries(Object.keys(v.canonical_subject).sort((a,b) => Buffer.compare(Buffer.from(a), Buffer.from(b))).map(key => [key,v.canonical_subject[key]]));
assert.equal(JSON.stringify(fields), v.canonical_utf8);
assert.equal('sha256:' + createHash('sha256').update(JSON.stringify(fields)).digest('hex'), v.canonical_digest);
const forged = Buffer.from(tag); forged[0] ^= 1; assert(!timingSafeEqual(tag, forged));
console.log('Node/Rust qualification UTF-8 framing, canonical digest and receipt vectors passed');

// Independent storage-runtime readiness vector: scope, bootstrap, policy, instance and body digest.
const r = JSON.parse(readFileSync(new URL('../fixtures/storage-runtime-ready-v1.json', import.meta.url)));
const sortKeys = value => {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(Object.keys(value).sort((a, b) => Buffer.compare(Buffer.from(a), Buffer.from(b))).map(key => [key, sortKeys(value[key])]));
  }
  return value;
};
const digestOf = text => 'sha256:' + createHash('sha256').update(text, 'utf8').digest('hex');
const readyMac = (keyHex, fields) => {
  const bytes = Buffer.concat(['oxide-runtime-ready-v1', ...fields].flatMap(value => {
    const encoded = Buffer.from(value, 'utf8');
    const size = Buffer.alloc(4); size.writeUInt32BE(encoded.length);
    return [size, encoded];
  }));
  return 'hmac-sha256-v1:' + createHmac('sha256', Buffer.from(keyHex, 'hex')).update(bytes).digest('hex');
};
const readyVerifies = (frame, expected) => {
  const { mac, ...body } = frame;
  const bodyDigest = digestOf(JSON.stringify(sortKeys(body)));
  const fields = [expected.scope, expected.bootstrap_digest, expected.policy_digest, body.storage_instance_id, bodyDigest];
  const computed = readyMac(expected.key_hex, fields);
  return computed.length === mac.length && timingSafeEqual(Buffer.from(computed), Buffer.from(mac));
};
assert.equal(digestOf(JSON.stringify(sortKeys(r.body))), digestOf(r.body_canonical_utf8));
assert.equal(r.body_canonical_utf8, JSON.stringify(sortKeys(r.body)));
assert(readyVerifies({ ...r.body, mac: r.mac }, r), 'ready vector authenticates');
assert.equal(BigInt(r.body.storage.device), 18446744073709551615n);
assert.equal(typeof r.body.storage.device, 'string');
assert.equal(BigInt(r.body.storage.inode), 9007199254740993n);
const refusals = {
  forged_mac: { ...r.body, mac: r.mac.slice(0, -1) + (r.mac.endsWith('0') ? '1' : '0') },
  device_substitution: { ...r.body, storage: { ...r.body.storage, device: '18446744073709551614' }, mac: r.mac },
  inode_substitution: { ...r.body, storage: { ...r.body.storage, inode: '9007199254740992' }, mac: r.mac },
  instance_substitution: { ...r.body, storage_instance_id: 'c'.repeat(64), mac: r.mac },
  budget_substitution: { ...r.body, budget: { cap_bytes: 1, free_reserve_bytes: 1048576 }, mac: r.mac },
};
for (const [name, frame] of Object.entries(refusals)) {
  assert(!readyVerifies(frame, r), name + ' must refuse');
}
assert(!readyVerifies({ ...r.body, mac: r.mac }, { ...r, scope: 'other-scope' }), 'wrong scope must refuse');
assert(!readyVerifies({ ...r.body, mac: r.mac }, { ...r, bootstrap_digest: digestOf('replayed bootstrap') }), 'replayed bootstrap must refuse');
assert(!readyVerifies({ ...r.body, mac: r.mac }, { ...r, policy_digest: digestOf('other policy') }), 'wrong policy must refuse');
assert(!readyVerifies({ ...r.body, mac: r.mac }, { ...r, key_hex: '77'.repeat(32) }), 'wrong control key must refuse');
console.log('Node/Rust storage-runtime ready vector authenticated; endpoint u64 text stays exact; substitutions and forgeries refuse');
