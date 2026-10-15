// Independent Node implementation of the Rust qualification framing contract.
import { readFileSync } from 'node:fs';
import { createHmac, createHash, timingSafeEqual } from 'node:crypto';
import assert from 'node:assert/strict';
function check(path) {
  const v = JSON.parse(readFileSync(new URL(path, import.meta.url)));
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
}
check('../fixtures/qualification-auth-v1.json');
check('../fixtures/qualification-graph-owner-v1.json');
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
const runtimePolicy = {
  ...r.policy_inputs,
  storage_protocol: 2,
  control_protocol: 1,
};
assert.equal(digestOf(JSON.stringify(sortKeys(runtimePolicy))), r.policy_digest);
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

// Cross-language ordinary-Cargo request and installed receipt vectors.
const ordinary = JSON.parse(readFileSync(new URL('../fixtures/ordinary-cargo-auth-v1.json', import.meta.url)));
const ordinaryCanonical = JSON.stringify(sortKeys(ordinary.request_subject));
assert.equal('sha256:' + createHash('sha256').update(ordinaryCanonical, 'utf8').digest('hex'), ordinary.request_digest);
assert.equal(digestOf(JSON.stringify(sortKeys(ordinary.input_boundary_subject))), ordinary.input_boundary_digest);
assert.equal(digestOf(JSON.stringify(sortKeys(ordinary.callback_context_subject))), ordinary.callback_context_digest);
const ordinarySettlementCanonical = JSON.stringify(sortKeys(ordinary.settlement.subject));
assert.equal(digestOf(ordinarySettlementCanonical), ordinary.settlement.subject_digest);
assert.equal(ordinary.settlement.request_digest, ordinary.request_digest);
const verifyMacVector = vector => {
  const frame = Buffer.concat([vector.domain, ...vector.fields].flatMap(value => {
    const encoded = Buffer.from(value, 'utf8');
    const size = Buffer.alloc(4); size.writeUInt32BE(encoded.length);
    return [size, encoded];
  }));
  assert.equal(frame.toString('hex'), vector.frame_hex);
  const receipt = 'hmac-sha256-v1:' + createHmac('sha256', Buffer.from(vector.key_hex, 'hex')).update(frame).digest('hex');
  assert.equal(receipt, vector.receipt);
};
for (const vector of [ordinary.admission, ordinary.callback, ordinary.settlement]) {
  verifyMacVector(vector);
}
console.log('Node/Rust ordinary input/request/settlement/context digests and admission/settlement/callback HMAC vectors passed');

// Owner-issued artifact-budget route v2: stable eligibility is separate from
// its exact attempt boundary and callback capture receipt.
const ordinaryV2 = JSON.parse(readFileSync(new URL('../fixtures/ordinary-cargo-auth-v2.json', import.meta.url)));
assert.equal(ordinaryV2.version, 2);
assert.equal(ordinaryV2.operation, 'ordinary-cargo-attempt-v2');
const budgetDigest = digestOf(JSON.stringify(sortKeys(ordinaryV2.capture_budget)));
assert.equal(budgetDigest, ordinaryV2.capture_budget_digest);
assert.equal(digestOf(JSON.stringify(sortKeys(ordinaryV2.input_boundary_subject))), ordinaryV2.input_boundary_digest);
assert.equal(digestOf(JSON.stringify(sortKeys(ordinaryV2.request_subject))), ordinaryV2.request_digest);
assert.equal(digestOf(JSON.stringify(sortKeys(ordinaryV2.callback_context_subject))), ordinaryV2.callback_context_digest);
assert.equal(digestOf(JSON.stringify(sortKeys(ordinaryV2.bootstrap))), ordinaryV2.bootstrap_digest);

const v2Input = ordinaryV2.bootstrap.invocation;
const invalidation = ordinaryV2.stable_invalidation_digest;
const stableMaterial = {
  schema: 'oxide.ordinary-cargo.stable-reuse.v2',
  policy_version: v2Input.acceptance.policy_version,
  source_root_path_hex: Buffer.from(v2Input.root.source_root, 'utf8').toString('hex'),
  consumer_root_id: v2Input.root.consumer_root_id,
  allocation_epoch: v2Input.root.allocation_epoch,
  ordinary_partition: v2Input.partition,
  project_component: v2Input.acquire.envelope.partition.project_component,
  workflow: v2Input.acquire.envelope.partition.workflow,
  cargo_provenance: v2Input.acquire.envelope.partition.cargo_provenance,
  rustc_provenance: v2Input.acquire.envelope.partition.rustc_provenance,
  host: v2Input.acquire.envelope.partition.host,
  target: v2Input.acquire.envelope.partition.target,
  profile: v2Input.acquire.envelope.partition.profile,
  features: [...v2Input.acquire.envelope.partition.features].sort(),
  config_digest: v2Input.acquire.envelope.partition.config_digest,
  environment_digest: v2Input.acquire.envelope.partition.environment_digest,
  path_identity: v2Input.acquire.envelope.partition.path_identity,
  installation_epoch: v2Input.acceptance.installation_epoch,
  qualified_invalidation_declaration_digest: invalidation,
};
const compatibility = digestOf(JSON.stringify(stableMaterial));
assert.equal(ordinaryV2.stable_reference,
  `oxide-ordinary-cargo-stable-eligibility-v2/${invalidation}/${compatibility}`);

const callbackRequest = ordinaryV2.callback_request;
assert.equal(callbackRequest.version, 2);
assert.equal(callbackRequest.operation, ordinaryV2.operation);
assert.equal(callbackRequest.capture_budget_digest, budgetDigest);
assert.equal(digestOf(JSON.stringify(sortKeys(callbackRequest.acquired))), callbackRequest.acquired_digest);
assert.equal(digestOf(JSON.stringify(sortKeys(callbackRequest.command))), callbackRequest.command_digest);
const launchSubject = {
  operation: callbackRequest.operation,
  request_digest: ordinaryV2.request_digest,
  branch_id: callbackRequest.source.branch_id,
  source_content_digest: callbackRequest.source.source_content_digest,
  selector: callbackRequest.command.selector,
  argv: callbackRequest.command.argv,
  cwd: callbackRequest.command.cwd,
  environment: callbackRequest.command.environment,
  target_dir: callbackRequest.command.target_dir,
  temp_dir: callbackRequest.command.temp_dir,
  capture_dir: callbackRequest.command.capture_dir,
  expected_outputs: callbackRequest.expected_outputs,
  operation_ids: v2Input.operation_ids,
  acquired: callbackRequest.acquired,
  capture_budget: callbackRequest.capture_budget,
  capture_budget_digest: callbackRequest.capture_budget_digest,
};
assert.equal(digestOf(JSON.stringify(sortKeys(launchSubject))), callbackRequest.launch_digest);
assert.equal(callbackRequest.run_identity.run_id, v2Input.acquire.envelope.binding.run_id);
assert.equal(ordinaryV2.callback_payload.capture.capture_budget_digest, budgetDigest);
assert.equal(ordinaryV2.callback_payload.capture.outputs[0].bytes, 64);
assert.equal(ordinaryV2.callback_reply.version, 2);
assert.equal(ordinaryV2.callback_reply.receipt, ordinaryV2.callback.receipt);

const verifyOrdinaryMac = vector => {
  const frame = Buffer.concat([vector.domain, ...vector.fields].flatMap(value => {
    const encoded = Buffer.from(value, 'utf8');
    const size = Buffer.alloc(4); size.writeUInt32BE(encoded.length);
    return [size, encoded];
  }));
  assert.equal(frame.toString('hex'), vector.frame_hex);
  const tag = createHmac('sha256', Buffer.from(vector.key_hex, 'hex')).update(frame).digest();
  assert.equal('hmac-sha256-v1:' + tag.toString('hex'), vector.receipt);
};
assert.equal(digestOf(JSON.stringify(sortKeys(ordinaryV2.callback_result_subject))), ordinaryV2.callback_result_digest);
assert.equal(ordinaryV2.callback.result_digest, ordinaryV2.callback_result_digest);
for (const vector of [ordinaryV2.admission, ordinaryV2.ordinary_admission, ordinaryV2.callback]) {
  verifyOrdinaryMac(vector);
}

const bootstrapFields = Object.keys(ordinaryV2.bootstrap).sort();
assert.deepEqual(bootstrapFields, [
  'callback_context', 'callback_key_hex', 'callback_timeout_ms', 'capture_budget',
  'invocation', 'operation', 'parent', 'remaining_ms', 'scope', 'storage', 'version',
]);
assert.deepEqual(Object.keys(ordinaryV2.bootstrap.invocation).sort(), [
  'acceptance', 'acquire', 'argv', 'branch_id', 'capture_dir', 'cwd', 'environment',
  'expected_outputs', 'operation_ids', 'partition', 'root', 'selector', 'source_content_digest',
  'target_dir', 'temp_dir',
]);
assert.deepEqual(Object.keys(callbackRequest).sort(), [
  'acquired', 'acquired_digest', 'capture_budget', 'capture_budget_digest', 'command',
  'command_digest', 'expected_outputs', 'id', 'launch_digest', 'method', 'operation',
  'run_identity', 'source', 'version',
]);
assert.equal(typeof ordinaryV2.capture_budget.total_bytes, 'number');
assert.equal(typeof ordinaryV2.bootstrap.storage.device, 'string');
assert.equal(BigInt(ordinaryV2.bootstrap.storage.device), 18446744073709551615n);
assert.equal(typeof ordinaryV2.bootstrap.storage.inode, 'string');
assert.equal(BigInt(ordinaryV2.bootstrap.storage.inode), 9007199254740993n);
assert.equal(typeof ordinaryV2.bootstrap.parent.inode, 'number');
assert.equal(JSON.stringify(ordinaryV2.bootstrap.storage.device), '"18446744073709551615"');
console.log('Node/Rust ordinary artifact-budget v2 bootstrap, stable identity, exact boundary, callback schema and HMAC vectors passed');

// Declared-workflow route v3: its own versioned boundary/context subject over
// the handoff-eligible policy-3 stable eligibility reference. Legacy v1/v2
// vectors above stay byte-for-byte unchanged.
const ordinaryV3 = JSON.parse(readFileSync(new URL('../fixtures/ordinary-cargo-auth-v3.json', import.meta.url)));
assert.equal(ordinaryV3.version, 3);
assert.equal(ordinaryV3.operation, 'ordinary-cargo-attempt-v3');
assert.equal(ordinaryV3.storage_protocol, 2);
const v3BudgetDigest = digestOf(JSON.stringify(sortKeys(ordinaryV3.capture_budget)));
assert.equal(v3BudgetDigest, ordinaryV3.capture_budget_digest);
const v3Input = ordinaryV3.bootstrap.invocation;
const v3Invalidation = ordinaryV3.stable_invalidation_digest;
const v3StableMaterial = {
  schema: 'oxide.ordinary-cargo.stable-reuse.v3',
  policy_version: v3Input.acceptance.policy_version,
  source_root_path_hex: Buffer.from(v3Input.root.source_root, 'utf8').toString('hex'),
  consumer_root_id: v3Input.root.consumer_root_id,
  allocation_epoch: v3Input.root.allocation_epoch,
  ordinary_partition: v3Input.partition,
  project_component: v3Input.acquire.envelope.partition.project_component,
  workflow: v3Input.acquire.envelope.partition.workflow,
  cargo_provenance: v3Input.acquire.envelope.partition.cargo_provenance,
  rustc_provenance: v3Input.acquire.envelope.partition.rustc_provenance,
  host: v3Input.acquire.envelope.partition.host,
  target: v3Input.acquire.envelope.partition.target,
  profile: v3Input.acquire.envelope.partition.profile,
  features: [...v3Input.acquire.envelope.partition.features].sort(),
  config_digest: v3Input.acquire.envelope.partition.config_digest,
  environment_digest: v3Input.acquire.envelope.partition.environment_digest,
  path_identity: v3Input.acquire.envelope.partition.path_identity,
  installation_epoch: v3Input.acceptance.installation_epoch,
  qualified_invalidation_declaration_digest: v3Invalidation,
};
// The stable eligibility material serializes in the Rust struct field order,
// not sorted key order; reproduce that exact encoding.
assert.equal(ordinaryV3.stable_reference,
  `oxide-ordinary-cargo-stable-eligibility-v3/${v3Invalidation}/${digestOf(JSON.stringify(v3StableMaterial))}`);
assert.equal(ordinaryV3.input_boundary_subject.boundary_version, 'oxide-ordinary-cargo-input-boundary-v3');
assert.equal(ordinaryV3.input_boundary_subject.declaration_digest, ordinaryV3.declaration_digest);
assert.equal(digestOf(JSON.stringify(sortKeys(ordinaryV3.input_boundary_subject))), ordinaryV3.input_boundary_digest);
assert.equal(digestOf(JSON.stringify(sortKeys(ordinaryV3.request_subject))), ordinaryV3.request_digest);
assert.equal(digestOf(JSON.stringify(sortKeys(ordinaryV3.callback_context_subject))), ordinaryV3.callback_context_digest);
assert.equal(ordinaryV3.callback_context_subject.operation, 'ordinary-cargo-attempt-v3');
assert.equal(digestOf(JSON.stringify(sortKeys(ordinaryV3.bootstrap))), ordinaryV3.bootstrap_digest);
assert.equal(ordinaryV3.bootstrap.declaration_digest, ordinaryV3.declaration_digest);
assert.equal(ordinaryV3.bootstrap.version, 3);

const v3CallbackRequest = ordinaryV3.callback_request;
assert.equal(v3CallbackRequest.version, 3);
assert.equal(v3CallbackRequest.operation, 'ordinary-cargo-attempt-v3');
assert.equal(v3CallbackRequest.capture_budget_digest, v3BudgetDigest);
assert.equal(digestOf(JSON.stringify(sortKeys(v3CallbackRequest.acquired))), v3CallbackRequest.acquired_digest);
assert.equal(digestOf(JSON.stringify(sortKeys(v3CallbackRequest.command))), v3CallbackRequest.command_digest);
const v3LaunchSubject = {
  operation: v3CallbackRequest.operation,
  request_digest: ordinaryV3.request_digest,
  branch_id: v3CallbackRequest.source.branch_id,
  source_content_digest: v3CallbackRequest.source.source_content_digest,
  selector: v3CallbackRequest.command.selector,
  argv: v3CallbackRequest.command.argv,
  cwd: v3CallbackRequest.command.cwd,
  environment: v3CallbackRequest.command.environment,
  target_dir: v3CallbackRequest.command.target_dir,
  temp_dir: v3CallbackRequest.command.temp_dir,
  capture_dir: v3CallbackRequest.command.capture_dir,
  expected_outputs: v3CallbackRequest.expected_outputs,
  operation_ids: v3Input.operation_ids,
  acquired: v3CallbackRequest.acquired,
  capture_budget: v3CallbackRequest.capture_budget,
  capture_budget_digest: v3CallbackRequest.capture_budget_digest,
};
assert.equal(digestOf(JSON.stringify(sortKeys(v3LaunchSubject))), v3CallbackRequest.launch_digest);
assert.equal(digestOf(JSON.stringify(sortKeys(ordinaryV3.callback_result_subject))), ordinaryV3.callback_result_digest);
assert.equal(ordinaryV3.callback_reply.version, 3);
assert.equal(ordinaryV3.callback_reply.receipt, ordinaryV3.callback.receipt);
for (const vector of [ordinaryV3.admission, ordinaryV3.ordinary_admission, ordinaryV3.callback]) {
  verifyOrdinaryMac(vector);
}
assert.deepEqual(Object.keys(ordinaryV3.bootstrap).sort(), [
  'callback_context', 'callback_key_hex', 'callback_timeout_ms', 'capture_budget',
  'declaration_digest', 'invocation', 'operation', 'parent', 'remaining_ms', 'scope',
  'storage', 'version',
]);
// A v2 signature/boundary is never reinterpreted as v3: the v2 vector keeps its
// own prefix and has no declaration digest or boundary version.
assert.ok(!('declaration_digest' in ordinaryV2.input_boundary_subject));
assert.ok(!('boundary_version' in ordinaryV2.input_boundary_subject));
assert.notEqual(ordinaryV2.operation, ordinaryV3.operation);
console.log('Node/Rust declared-workflow v3 bootstrap, versioned boundary/context subject, exact digests and HMAC vectors passed');
