Fixed `effigy oxide:canary` failing on the nested removable layout: the live-canary
scratch check now accepts a registered payload strictly beneath the mounted
volume (such as `<mount>/builds/oxide/pool`) instead of only a flat `<mount>`
child, while still refusing a payload equal to, outside, or escaping the mount.
