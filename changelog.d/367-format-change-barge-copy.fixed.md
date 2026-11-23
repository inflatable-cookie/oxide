Fixed `--canary --format-change` refusing Barge-backed internal pools with
`unknown_accounting`: the scratch document copy now carries an empty owner-only
`payload/slots/` directory, so the staged binary opens the copied `config.json`,
`ledger.json` and `oxide-managed/state.json`, initializes the copy's own Barge
state, and upgrades the copied control state, all without writing the live pool.
