Fixed `--canary --format-change` failing when a removable pool is registered: the
internal-pool copy and its canary crate now live on the internal pool's volume
(`--scratch-root`, or the pool's sibling `scratch/` by default) instead of the
removable volume's `suggested_scratch_root`, which the staged binary refuses as
an explicit internal pool on an external volume.
