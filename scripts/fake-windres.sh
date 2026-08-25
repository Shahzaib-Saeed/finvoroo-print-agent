#!/bin/sh
# Stub resource compiler so `cargo check --target x86_64-pc-windows-gnu` can run on
# macOS/Linux. Type-checking never links, so an empty .lib is enough. Not used by CI.
case "$1" in
  -V|--version|/?)
    echo "GNU windres (fake shim) 2.0"
    exit 0
    ;;
esac

out=""
prev=""
for arg in "$@"; do
  [ "$prev" = "--output" ] && out="$arg"
  prev="$arg"
done

[ -n "$out" ] && : > "$out"
exit 0
