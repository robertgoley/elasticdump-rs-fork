#!/usr/bin/env bash
# progress-add.sh TASK STATUS COMMIT "TESTS" "NOTES"
printf '| %s | %s | %s | %s | %s |\n' "$1" "$2" "$3" "$4" "$5" >> /workspace/elasticdump-rs-plan/PROGRESS.md
