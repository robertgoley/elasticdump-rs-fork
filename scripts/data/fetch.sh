#!/usr/bin/env bash
# Dataset cache manager (TST-INF-008 / TST-DATA-004). See fetch.py.
exec python3 "$(dirname "$0")/fetch.py" "$@"
