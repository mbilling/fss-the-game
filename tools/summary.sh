#!/bin/sh
# Run every reference plan and print the verdict lines (tuning aid).
cd "$(dirname "$0")/.." || exit 2
cargo build -q -p gridlock-cli || exit 2
for p in balanced unsecured no_storage all_import; do
  out=$(./target/debug/gridlock run scenarios/cloudy_tuesday/cloudy_tuesday.toml --plan tests/golden/plans/$p.toml)
  code=$?
  printf '== %-11s exit %s | %s\n' "$p" "$code" "$(echo "$out" | head -1 | sed 's/.*budget //;s/ |.*//')"
  echo "$out" | sed -n '/Stars lost/,/^$/p' | sed 's/^/   /'
  echo "$out" | grep -E '^(PASS|FAIL|Energy)' | sed 's/^/   /'
done
