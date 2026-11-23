On macOS, idle `oxide exec` sessions yield their host build permit while keeping their slot. Nested Oxide builds borrow one shared permit on demand, and `oxide status` shows when it is borrowed.
