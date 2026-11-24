Bound the Barge slot label length by the pool's slots directory path so
`<slots>/<label>-<16 hex>/scratch` fits within the 91-byte Unix-socket
bound: a 38-byte nested slots dir like `/Volumes/Stash/builds/oxide/pool/slots`
now yields at most a 27-byte label instead of 32. The 91-byte constant lives in
the lease library with the child-temporary decision mirroring it, and the
fallback label is truncated to fit tiny budgets. Existing longer labels still
validate and are never renamed.
