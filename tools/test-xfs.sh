#!/usr/bin/env bash
# Destructive only to two newly created disposable loopback images.
# Never run on production; does not accept a device or existing mount path.
set -euo pipefail
if [[ ${1:-} != --isolated-test-host || ${XELAB_XFS_TEST:-} != 1 || $EUID != 0 || $# != 2 ]]; then
  echo 'Usage (isolated host only): XELAB_XFS_TEST=1 sudo -E bash tools/test-xfs.sh --isolated-test-host TEST_BINARY' >&2
  exit 2
fi
binary=$(realpath -- "$2")
test -x "$binary"
for tool in mkfs.xfs mount umount truncate; do command -v "$tool" >/dev/null; done
work=$(mktemp -d /mnt/xelab-xfs.XXXXXX)
cleanup() {
  local rc=$?
  trap - EXIT
  for name in readonly accounting enforced; do
    if mountpoint -q "$work/$name"; then
      if ! umount "$work/$name"; then
        echo "Cleanup failed: isolated test mount retained at $work/$name" >&2
        exit 1
      fi
    fi
  done
  rm -f -- "$work/enforced.img" "$work/accounting.img"
  rmdir -- "$work/enforced" "$work/accounting" "$work/readonly" "$work"
  exit "$rc"
}
mkdir -m 700 "$work/enforced" "$work/accounting" "$work/readonly"
trap cleanup EXIT
for name in enforced accounting; do
  truncate -s 512M "$work/$name.img"
  mkfs.xfs -q "$work/$name.img"
done
mount -o loop,prjquota,nosuid,nodev,noexec "$work/enforced.img" "$work/enforced"
mount -o loop,pqnoenforce,nosuid,nodev,noexec "$work/accounting.img" "$work/accounting"
chmod 700 "$work/enforced" "$work/accounting"
mount --bind "$work/enforced" "$work/readonly"
mount -o remount,bind,ro "$work/readonly"
export XELAB_XFS_READONLY_ROOT="$work/readonly"
export XELAB_XFS_ROOT="$work/enforced"
export XELAB_XFS_ACCOUNTING_ROOT="$work/accounting"
"$binary" --ignored --nocapture
