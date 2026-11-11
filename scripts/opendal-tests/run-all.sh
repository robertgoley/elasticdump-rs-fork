#!/usr/bin/env bash
# L-809a: every local OpenDAL storage test: fs, S3 (MinIO and RustFS, SSE-C), SFTP (OpenSSH
# and Dropbear) and the cross-store/cluster run. Real B2 is separate (b2-real.sh).
# Build first: cargo build --bins. ODT_OS_PODMAN_ARGS: see cross.sh.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
rc=0
for t in "fs.sh" "s3.sh minio" "s3.sh rustfs" "sftp.sh openssh" "sftp.sh dropbear" "cross.sh"; do
  echo "=== $t"
  # shellcheck disable=SC2086
  "$here"/$t | grep -E '^(== |  (PASS|FAIL))' || rc=1
  [ "${PIPESTATUS[0]}" -eq 0 ] || rc=1
done
exit $rc
