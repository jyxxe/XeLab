#!/usr/bin/env bash
# Destructive only to newly created disposable loopback images.
# Never run on production; does not accept a device or existing mount path.
set -euo pipefail
if [[ ${1:-} != --isolated-test-host || ${XELAB_XFS_TEST:-} != 1 || $EUID != 0 || ( $# != 2 && $# != 3 && $# != 4 ) ]]; then
  echo 'Usage (isolated host only): XELAB_XFS_TEST=1 sudo -E bash tools/test-xfs.sh --isolated-test-host TEST_BINARY' >&2
  exit 2
fi
binary=$(realpath -- "$2")
test -x "$binary"
for tool in mkfs.xfs mount umount truncate findmnt; do command -v "$tool" >/dev/null; done
work=$(mktemp -d /mnt/xelab-xfs.XXXXXX)
cleanup() {
  local rc=$?
  trap - EXIT
  # Artifact inspection can mount read-only OCI views inside this newly
  # created service directory. Unmount these owned views before deleting any
  # test files; never recurse through a live image or volume mount.
  local target
  local -a service_mounts
  mapfile -t service_mounts < <(findmnt -rn -o TARGET | awk -v prefix="$work/service/" -v recovery="$work/platform-target/" 'index($0,prefix)==1 || index($0,recovery)==1' | sort -r)
  for target in "${service_mounts[@]}"; do
    if ! umount "$target"; then
      echo "Cleanup failed: isolated test mount retained at $target" >&2
      exit 1
    fi
  done
  for name in platform-target/volumes restore-service/volumes service/volumes readonly accounting enforced; do
    if mountpoint -q "$work/$name"; then
      if ! umount "$work/$name"; then
        echo "Cleanup failed: isolated test mount retained at $work/$name" >&2
        exit 1
      fi
    fi
  done
  rm -f -- "$work/enforced.img" "$work/accounting.img" "$work/service.img" "$work/restore.img" "$work/platform.img"
  rm -rf --one-file-system -- "$work/service" "$work/restore-service" "$work/platform-target"
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

if [[ $# -ge 3 ]]; then
  volume_binary=$(realpath -- "$3")
  test -x "$volume_binary"
  mkdir -m 700 "$work/service" "$work/service/volumes"
  truncate -s 512M "$work/service.img"
  mkfs.xfs -q "$work/service.img"
  mount -o loop,prjquota,nosuid,nodev,noexec "$work/service.img" "$work/service/volumes"
  chmod 700 "$work/service/volumes"
  export XELAB_VOLUME_TEST_ROOT="$work/service"
  "$volume_binary" --ignored --nocapture
fi

if [[ $# == 4 ]]; then
  restore_binary=$(realpath -- "$4")
  test -x "$restore_binary"
  mkdir -m 700 "$work/restore-service" "$work/restore-service/volumes"
  truncate -s 512M "$work/restore.img"
  mkfs.xfs -q "$work/restore.img"
  mount -o loop,prjquota,nosuid,nodev,noexec "$work/restore.img" "$work/restore-service/volumes"
  chmod 700 "$work/restore-service/volumes"
  export XELAB_RESTORE_TEST_ROOT="$work/restore-service"
  "$restore_binary" --ignored --nocapture --exact volumes::restore_tests::actual_private_restore_allocator_and_verified_files
  mkdir -m 700 "$work/platform-target" "$work/platform-target/volumes"
  truncate -s 512M "$work/platform.img"
  mkfs.xfs -q "$work/platform.img"
  mount -o loop,prjquota,nosuid,nodev,noexec "$work/platform.img" "$work/platform-target/volumes"
  chmod 700 "$work/platform-target/volumes"
  export XELAB_PLATFORM_RESTORE_ROOT="$work/platform-target"
  "$restore_binary" --ignored --nocapture --exact offline::restore::tests::actual_platform_recovery_rebuilds_fresh_state_and_resumes_after_process_death
fi
