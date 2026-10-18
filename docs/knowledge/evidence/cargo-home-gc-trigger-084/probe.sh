#!/bin/bash
# oxide#084 single confirmation session: Cargo 1.97.1 automatic home-cache GC trigger.
# Fresh scratch CARGO_HOME only. Exactly 6 Cargo invocations. Not a product file.

S=/private/tmp/oxide-084.Df89lF
R=$S/run
TC=/Users/tom/.rustup/toolchains/1.97.1-aarch64-apple-darwin/bin
mkdir -m 700 "$R" "$R/home" "$R/proj" "$R/proj/src" "$R/target" "$R/logs" "$R/tmp" || exit 90
cat > "$R/proj/Cargo.toml" <<'T'
[package]
name = "gc-probe"
version = "0.1.0"
edition = "2021"

[dependencies]
cfg-if = "=1.0.0"
T
echo 'cfg_if::cfg_if! { if #[cfg(unix)] { pub fn f() {} } else { pub fn f() {} } }' > "$R/proj/src/lib.rs"
ENV=(env -i HOME="$HOME" PATH="$TC:/usr/bin:/bin" CARGO_HOME="$R/home" CARGO_TARGET_DIR="$R/target"
     RUSTC="$TC/rustc" TMPDIR="$R/tmp" TMP="$R/tmp" TEMP="$R/tmp" CARGO_TERM_COLOR=never CARGO_LOG=gc=trace)
DEAD=(CARGO_HTTP_PROXY=http://127.0.0.1:9 CARGO_NET_RETRY=0)
ALWAYS=(--config 'cache.auto-clean-frequency="always"')
n=0
run() { # name, extra env..., -- cargo args...
  local name=$1; shift; local extra=()
  while [ "$1" != "--" ]; do extra+=("$1"); shift; done; shift
  n=$((n+1)); [ $n -le 6 ] || { echo "CAP $name refused"; return; }
  local t0=$(date -u +%s)
  echo "CALL $n $name START $(date -u +%FT%TZ) argv: cargo $*  extra-env: ${extra[*]:-none}"
  (cd "$R/proj" && "${ENV[@]}" "${extra[@]}" perl -e 'alarm 120; exec @ARGV' "$TC/cargo" "$@") > "$R/logs/$n-$name.log" 2>&1
  local rc=$?
  echo "CALL $n $name EXIT $rc $(( $(date -u +%s) - t0 ))s"
  db
}
db() {
  if [ -f "$R/home/.global-cache" ]; then
    echo "  last_auto_gc=$(sqlite3 -readonly "$R/home/.global-cache" 'SELECT last_auto_gc FROM global_data') now=$(date -u +%s)"
    echo "  registry_crate=$(sqlite3 -readonly "$R/home/.global-cache" 'SELECT name,size,timestamp FROM registry_crate' | tr '\n' ' ')"
  else echo "  no .global-cache"; fi
  echo "  home_kib=$(du -sk "$R/home" | cut -f1) df_avail_gib=$(df -g / | tail -1 | awk '{print $4}')"
}
echo "SESSION START $(date -u +%FT%TZ)"; db
run version -- -V
run fetch-online-default -- fetch
run fetch-offline-always -- fetch --offline "${ALWAYS[@]}"
run check-frozen-always "${DEAD[@]}" -- check --frozen "${ALWAYS[@]}"
sleep 2
run fetch-online-deadproxy-always "${DEAD[@]}" -- fetch "${ALWAYS[@]}"
sleep 2
run check-online-deadproxy-always "${DEAD[@]}" -- check "${ALWAYS[@]}"
echo "--- crate archives"
find "$R/home/registry/cache" -name '*.crate' -exec stat -f '%z %N' {} \; -exec shasum -a 256 {} \;
grep -A3 'name = "cfg-if"' "$R/proj/Cargo.lock"
echo "--- home tree (depth 3)"; find "$R/home" -maxdepth 3 | sed "s|$R/||" | sort
echo "SESSION END $(date -u +%FT%TZ)"
