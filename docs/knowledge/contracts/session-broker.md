# Session broker

On macOS and Linux, a leased command hosts a session broker for nested Oxide commands.
Each joined command takes its own exclusive slot. A build can inherit an
active outer permit or borrow one through an `oxide exec` session. See
[architecture](../architecture.md) for ownership and [workflow](workflow.md)
for the command budgets.

## Endpoint and authentication

The outer process binds a Unix domain socket inside a fresh, per-session
directory with mode `0700` under the leased child's [chosen temporary root](workflow.md#child-temporary-files).
Before creating it, Oxide checks the canonical path from `/` down. Each
directory must be owned by the current user or root on a volume that enforces
ownership. A directory writable by its group or by others must be sticky.
On macOS, any ACL grant refuses the path except a deny-only ACL. On Linux, the
volume must positively establish ownership enforcement: it must be one of
ext2/3/4, XFS, Btrfs, tmpfs or overlayfs, as reported by `statfs`. Any other
filesystem type refuses the path, including network, FUSE, CIFS, SMB, FAT and
exFAT volumes. Idmapped mounts are not inspected, so they are not established
by this check.

On Linux, the access ACL (`system.posix_acl_access`) is parsed as the version 2
xattr encoding, with `ACL_MASK` applied to named users, the owning group and
named groups. Entries for the owner, the root user and the effective user are
trusted. An ancestor refuses if any other entry is effectively writable. It may
grant read and traverse, as `/home` often does. The private socket directory
refuses if any other entry is effectively readable, writable or executable. A
malformed ACL, an unknown version, or a truncated attribute also refuses, and so
does an ACL that changes between the size probe and the read. A default ACL
shapes children that are created later, so the private socket directory's own
access ACL is checked after creation to cover it.

These checks protect the pathname used for binding and cleanup from replacement
by another user.

Oxide also checks the socket's absolute pathname before creation. `sun_path`
includes the terminating NUL: 104 bytes on macOS, which leaves at most 103 bytes
for the path, and 108 bytes on Linux, which leaves at most 107. An overlong path
refuses the launch.

The broker supplies two variables for the outer command to pass to its children.

| Variable | Value |
| --- | --- |
| `OXIDE_SESSION_BROKER` | Absolute socket path. |
| `OXIDE_SESSION_TOKEN` | 256-bit token generated with OS randomness and encoded as 64 lowercase hex digits. Treat it as a secret. |

On macOS and Linux each side checks the other's effective UID before exchanging
protocol data: `getpeereid` on macOS and `SO_PEERCRED` on Linux. The UID must
equal the local `geteuid()`. The broker also compares the initial frame's token
in constant time after checking its length. The socket directory and peer check
limit access to the local UID. The token binds a connection to the advertised
session within that trust boundary. On other Unix platforms, peer verification
fails closed with an unsupported-platform error, and the broker neither binds
nor joins.

A leased invocation given either variable requires both and authenticates
before opening the pool. A missing, invalid, unreachable or refusing broker
ends the invocation. Help, passthrough and read-only commands leave the broker
alone. A joined command hosts its own broker for its leased children so a
deeper invocation inherits the tighter deadline too.

## Protocol version 1

Each frame is a UTF-8 JSON object followed by LF, with a maximum of 4096 bytes
including LF. It has integer `version: 1` and a `body` object tagged by `type`.
Authentication failure, malformed or oversized frames, unknown versions and
requests invalid for the connection state close the socket without a reply.
A policy refusal of an authenticated permit request returns `permit_denied`.

| Request body | Response body | Lifetime |
| --- | --- | --- |
| `join { token }` | `joined { session_id, member_id, remaining_execution_ms, permit_mode }` | The socket owns the membership. `permit_mode` is `inherited` or `borrow_required`. |
| `borrow_permit { token, admission_ms? }` | `permit_granted { borrow_id, remaining_execution_ms }` or `permit_denied { reason }` | A separate socket owns the borrowed permit until close. Denial reasons are `unavailable`, `busy` and `deadline_expired`. |
| `release { member_id }` | `released` | Releases the member on its original joined socket, then closes that socket. |

The session ID is an opaque string. Member and borrow IDs are unsigned integers
scoped to that session. A join with `inherited` uses the outer permit.
`borrow_required` tells a nested build to obtain a separate grant after it
gets a slot and before it binds outputs or starts Cargo. A nested `exec` can
start its tool without borrowing and relays its descendants' requests
upstream. An `exec` that inherits an already-active build permit passes that
permit through to its children.

An independent `exec` yields its startup permit after it records the tool's
process group. Its first pending borrow competes in the pool's normal host
admission queue, skipping waiters blocked on checkout or slot custody. The
broker shares the same permit across concurrent grants and returns it when
the last grant closes. Plain Cargo launched by a tool still uses the tool's
slot under the cooperative storage contract. It does not request a host
permit because Oxide does not intercept it.

The broker starts accepting handshakes once the parent publishes the outer
execution deadline, or explicitly marks execution unbounded. The deadline is
set once. `remaining_execution_ms` is sampled against that monotonic deadline
when the response is sent. It is an unsigned integer rounded down, with zero
meaning no usable time remains and JSON `null` meaning unbounded. Nested
admission and execution each use the tighter of their own budget and the
remaining outer window. The client anchors the reported interval before the
handshake, then retains that absolute cap through admission and execution.
The optional `admission_ms` in a borrow request carries the caller's
remaining admission budget. The broker also caps its wait to its execution
deadline. The client cancels by closing the pending socket at its own
deadline or on interruption. A permit grant does not restart either budget.
A zero join window refuses before pool or slot admission with exit 124.
Expiry while waiting for a permit also exits 124 without starting Cargo.
The outer process-group kill remains the hard backstop.

## Close and crash

EOF or a socket failure releases the connection's member or borrowed permit
automatically. A disconnected pending request also stops competing for a
permit. Connection cleanup gives no authority to release a storage hold
or assume a process group has stopped writing. On normal shutdown the broker
stops accepting, shuts down all client sockets, drains connection cleanup, and
removes the socket and its private directory. An already-connected child
treats EOF or connection loss as the end of its outer session. Oxide refuses
further slot admission and stops any group it is supervising through its normal
bounded closure path. A process death
closes the sockets through the OS and may leave the private directory on disk.
Each new session creates its own directory.
