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
