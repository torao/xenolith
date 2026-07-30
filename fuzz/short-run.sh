#!/usr/bin/env bash
# A short fuzzing run of every target, seeded from the corpus in the repository.
#
# This is what CI runs on every push and what one runs locally before pushing, so that the two
# are the same thing rather than two scripts that drift apart. It will not exhaust anything: a
# minute a target is enough to say the targets still build and that nothing obvious crashes.
# For a real session, give one target as much time as you have:
#
#     cargo fuzz run parse_document -- -max_total_time=3600
#
# Needs a nightly toolchain and cargo-fuzz. On Windows the libFuzzer runtime does not load, so
# run it under WSL or on Linux; the properties themselves are checked everywhere by
# `cargo test -p xylograph-fuzz`.
set -euo pipefail

seconds="${1:-60}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname "$here")"
toolchain="${FUZZ_TOOLCHAIN:-nightly}"

# Which seeds each target starts from: bytes that are meant to be a document, an expression, or
# a stylesheet. Starting a stylesheet target from documents would waste the run rediscovering
# what a stylesheet looks like.
seeds_for() {
  case "$1" in
    compile_expression) echo expressions ;;
    transform) echo stylesheets ;;
    *) echo documents ;;
  esac
}

targets="parse_document build_and_serialize validate_document compile_expression transform"

for target in $targets; do
  mkdir -p "$root/fuzz/corpus/$target"
  cp "$root/crates/xylograph-fuzz/corpus/$(seeds_for "$target")"/* "$root/fuzz/corpus/$target/"
  echo "===== $target ====="
  (cd "$root" && cargo "+$toolchain" fuzz run "$target" -- -max_total_time="$seconds" -print_final_stats=1)
done
