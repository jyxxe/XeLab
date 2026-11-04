#!/usr/bin/env bash
# Actual AWS writes, signed OCI and a NEW disposable XFS loop image only.
# Never accepts an existing device/mount or creates/deletes a bucket.
set -euo pipefail
if [[ ${1:-} != --isolated-test-host || $# != 2 || $EUID != 0 ]]; then
  echo 'Usage on an authorized disposable host: tools/test-aws-format.sh --isolated-test-host VOLUMES_TEST_BINARY' >&2
  exit 2
fi
for variable in XELAB_TEST_DATABASE_URL AWS_REGION XELAB_BACKUP_CREDENTIALS_FILE XELAB_AWS_FORMAT_URI XELAB_AWS_FORMAT_NAMESPACE XELAB_TEST_AWS_FORMAT_SOURCE_IMAGE XELAB_TEST_AWS_FORMAT_TARGET_IMAGE; do
  if [[ -z ${!variable:-} ]]; then echo "Required configuration missing: $variable" >&2; exit 2; fi
done
if [[ -v XELAB_TEST_S3_ENDPOINT || $XELAB_AWS_FORMAT_NAMESPACE != xelab-aws-format-* ]]; then
  echo 'Require the actual AWS backend and a dedicated xelab-aws-format-* namespace' >&2
  exit 2
fi
if [[ ! $XELAB_AWS_FORMAT_URI =~ ^s3://[^/]+/[^/]+ ]]; then
  echo 'Provide an existing test bucket and an explicitly authorized nonempty test prefix' >&2
  exit 2
fi
for tool in mkfs.xfs mount umount mountpoint findmnt truncate ctr; do command -v "$tool" >/dev/null; done
binary=$(realpath -- "$2")
test -x "$binary"
case_name=actual_aws_backed_format_upgrade_unknown_apply_and_incompatible_rollback
listed=$("$binary" --list)
if [[ $listed != *"$case_name: test"* ]]; then
  echo 'The binary does not contain the actual AWS format test' >&2
  exit 2
fi
test -f "$XELAB_BACKUP_CREDENTIALS_FILE"
test -f examples/generated-aws-source/xelab/plugin.json
test -f examples/generated-aws-target/xelab/plugin.json
if [[ -n $(ctr -n "$XELAB_AWS_FORMAT_NAMESPACE" tasks list -q) ]]; then
  echo 'The dedicated test namespace already has workloads; refusing to touch them' >&2
  exit 2
fi
work=$(mktemp -d /mnt/xelab-aws-format.XXXXXX)
evidence=$(mktemp -d /root/xelab-aws-format-evidence.XXXXXX)
chmod 700 "$work" "$evidence"
cleanup() {
  local rc=$?
  trap - EXIT
  local target
  local -a owned_mounts
  mapfile -t owned_mounts < <(findmnt -rn -o TARGET | awk -v prefix="$work/service/" 'index($0,prefix)==1' | sort -r)
  for target in "${owned_mounts[@]}"; do
    if ! umount "$target"; then
      echo "Owned AWS test mount retained for inspection: $target" >&2
      exit 1
    fi
  done
  if mountpoint -q "$work/service/volumes"; then umount "$work/service/volumes"; fi
  rm -f -- "$work/volume.img"
  rm -rf --one-file-system -- "$work/service"
  rmdir -- "$work"
  echo "Private AWS test master key/receipts retained at $evidence" >&2
  exit "$rc"
}
trap cleanup EXIT
mkdir -m 700 "$work/service" "$work/service/volumes"
truncate -s 512M "$work/volume.img"
mkfs.xfs -q "$work/volume.img"
mount -o loop,prjquota,nosuid,nodev,noexec "$work/volume.img" "$work/service/volumes"
chmod 700 "$work/service/volumes"
export XELAB_AWS_FORMAT_TEST=1 XELAB_XFS_TEST=1
export XELAB_VOLUME_TEST_ROOT="$work/service"
export XELAB_AWS_FORMAT_EVIDENCE_DIR="$evidence"
"$binary" --ignored --nocapture --exact "$case_name"
test -s "$evidence/completed.json"
