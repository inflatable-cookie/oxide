Fixed `oxide pool relocate` refusing nested paths under a user-owned readable
shared parent: intermediate directories such as `0755 builds/` now pass a
not-group-or-world-writable check, while the payload and outputs directories
themselves stay owner-only (`0700`). Group- or world-writable parents still
refuse.
