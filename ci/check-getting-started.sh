#!/usr/bin/env bash
#
# Execute the command transcript in docs/getting-started.md, so the guide can
# never drift from what the CLI actually does.
#
# How it works. The guide marks each runnable ```console block with an HTML
# comment on the line directly above it:
#
#   <!-- exec -->              every `$ ` command in the block must exit 0
#   <!-- exec allow-fail -->   commands may exit non-zero (error/finding demos)
#
# All the marked blocks are run, top to bottom, as ONE shell session in a
# throwaway sandbox: `cd` and shell variables persist across blocks, exactly as a
# reader following along would experience. Only `$ `-prefixed lines are executed;
# the expected-output lines are ignored (IDs are random and paths are absolute,
# so matching them verbatim would be noise). A `$` command that exits non-zero in
# a strict block fails the run and prints the offending command.
#
# The `colophon` name in the transcript resolves to the built binary (below), and
# COLOPHON_QUIET silences the config-advisory line so it can't interleave with
# asserted output.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUIDE="$ROOT/docs/getting-started.md"

# Locate the binary: an explicit COLOPHON_BIN wins; otherwise prefer a release
# build, then a debug build, then build a debug one on the spot.
if [[ -n "${COLOPHON_BIN:-}" ]]; then
  BIN="$COLOPHON_BIN"
elif [[ -x "$ROOT/target/release/colophon" ]]; then
  BIN="$ROOT/target/release/colophon"
elif [[ -x "$ROOT/target/debug/colophon" ]]; then
  BIN="$ROOT/target/debug/colophon"
else
  echo "building colophon (debug) ..."
  ( cd "$ROOT" && cargo build -p colophon-cli ) || exit 1
  BIN="$ROOT/target/debug/colophon"
fi
echo "using binary: $BIN"

# The transcript writes `colophon ...`; route it to the built binary. Defined as
# a function so command substitution (`$(colophon id …)`) inherits it too.
colophon() { "$BIN" "$@"; }
export COLOPHON_QUIET=1

SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT
cd "$SANDBOX"

ran=0
failed=0
mode="none"     # none | strict | allowfail  (the block currently open)
pending=""      # a marker seen, waiting for the block it applies to
inblock=0

while IFS= read -r line; do
  # Marker lines arm the *next* console block.
  if [[ "$line" == '<!-- exec -->' ]]; then pending="strict"; continue; fi
  if [[ "$line" == '<!-- exec allow-fail -->' ]]; then pending="allowfail"; continue; fi

  # Fence toggles a block. An opening fence consumes any pending marker.
  if [[ "$line" == '```'* ]]; then
    if (( inblock )); then
      inblock=0; mode="none"
    elif [[ -n "$pending" ]]; then
      inblock=1; mode="$pending"; pending=""
    fi
    continue
  fi

  # A non-blank, non-fence line before a block clears a stray marker.
  if [[ -n "$pending" && -n "${line// }" ]]; then pending=""; fi

  (( inblock )) || continue
  [[ "$mode" == "none" ]] && continue
  [[ "$line" == '$ '* ]] || continue   # only command lines; skip expected output

  cmd="${line#'$ '}"
  ran=$(( ran + 1 ))
  if eval "$cmd"; then
    :
  else
    rc=$?
    if [[ "$mode" == "strict" ]]; then
      echo "TRANSCRIPT FAILED (exit $rc): $cmd" >&2
      failed=$(( failed + 1 ))
    fi
  fi
done < "$GUIDE"

if (( failed )); then
  echo "getting-started transcript: $failed command(s) failed of $ran run" >&2
  exit 1
fi
echo "getting-started transcript: all $ran command(s) ran clean"
