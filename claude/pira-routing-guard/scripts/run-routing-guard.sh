#!/usr/bin/env bash

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
script="$script_dir/pira_routing_guard.py"

if command -v python3 >/dev/null 2>&1 && python3 -c "import sys; raise SystemExit(sys.version_info < (3, 10))" >/dev/null 2>&1; then
    exec python3 "$script" "$@"
fi
if command -v python >/dev/null 2>&1 && python -c "import sys; raise SystemExit(sys.version_info < (3, 10))" >/dev/null 2>&1; then
    exec python "$script" "$@"
fi
if command -v py.exe >/dev/null 2>&1 && py.exe -3 -c "import sys; raise SystemExit(sys.version_info < (3, 10))" >/dev/null 2>&1; then
    exec py.exe -3 "$script" "$@"
fi
if command -v py >/dev/null 2>&1 && py -3 -c "import sys; raise SystemExit(sys.version_info < (3, 10))" >/dev/null 2>&1; then
    exec py -3 "$script" "$@"
fi

echo "PIRA routing guard unavailable: no Python 3.10+ interpreter found (checked python3, python, py -3)" >&2
exit 1
