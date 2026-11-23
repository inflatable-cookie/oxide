# Session broker

On macOS, a leased command hosts a session broker for nested Oxide commands.
Each joined command takes its own exclusive slot under the outer command's
host permit. Idle `oxide exec` permit borrowing is a protocol capability for a
later slice. See [architecture](../architecture.md) for ownership and
[workflow](workflow.md) for the command budgets.

## Endpoint and authentication

The outer process binds a Unix domain socket inside a fresh, per-session
directory with mode `0700` under the leased child's [chosen temporary root](workflow.md#child-temporary-files).
The root must have trusted ancestors. Oxide checks the socket's absolute
pathname before creating the directory. macOS has 104 bytes in `sun_path`
including the terminating NUL, which leaves at most 103 bytes for the path.
An overlong path refuses the launch.

The broker supplies two variables for the outer command to pass to its children.

| Variable | Value |
| --- | --- |
| `OXIDE_SESSION_BROKER` | Absolute socket path. |
| `OXIDE_SESSION_TOKEN` | 256-bit token generated with OS randomness and encoded as 64 lowercase hex digits. Treat it as a secret. |

On macOS each side checks the other's effective UID with `getpeereid` before
exchanging protocol data. The UID must equal the local `geteuid()`. The broker
also compares the initial frame's token in constant time after checking its
length. The socket directory and peer check limit access to the local UID.
The token binds a connection to the advertised session within that trust
boundary. On other platforms, including Linux, peer verification fails closed
with an unsupported-platform error.

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
| `borrow_permit { token }` | `permit_granted { borrow_id, remaining_execution_ms }` or `permit_denied { reason }` | A separate socket owns the borrowed permit. Denial reasons are `unavailable`, `busy` and `deadline_expired`. |
| `release { member_id }` | `released` | Releases the member on its original joined socket, then closes that socket. |

The session ID is an opaque string. Member and borrow IDs are unsigned integers
scoped to that session. A join with `inherited` uses the outer permit.
`borrow_required` tells the member to obtain a separate grant before using a
permit. The parent's permit controller grants or denies a borrow and receives
its return when the grant ends. The current command path grants inherited
membership and denies borrow requests. A command refuses `borrow_required`
until borrowing is available.

The broker starts accepting handshakes once the parent publishes the outer
execution deadline, or explicitly marks execution unbounded. The deadline is
set once. `remaining_execution_ms` is sampled against that monotonic deadline
when the response is sent. It is an unsigned integer rounded down, with zero
meaning no usable time remains and JSON `null` meaning unbounded. Nested
admission and execution each use the tighter of their own budget and the
remaining outer window. The client anchors the reported interval before the
handshake, then retains that absolute cap through admission and execution.
Zero refuses before pool or slot admission with exit 124. The outer
process-group kill remains the hard backstop.

## Close and crash

EOF or a socket failure releases the connection's member or borrowed permit
automatically. Connection cleanup gives no authority to release a storage hold
or assume a process group has stopped writing. On normal shutdown the broker
stops accepting, shuts down all client sockets, drains connection cleanup, and
removes the socket and its private directory. An already-connected child
treats EOF or connection loss as the end of its outer session. Oxide refuses
further slot admission and stops any group it is supervising through its normal
bounded closure path. A process death
closes the sockets through the OS and may leave the private directory on disk.
Each new session creates its own directory.
