#!/usr/bin/env bash
# Build the actual platform images used by the disposable-host M3 browser test.
set -euo pipefail
umask 077
if [[ $# -ne 3 || "$1" != "--isolated-test-host" || "$2" != "--env-file" ]]; then
  echo 'usage: prepare-m3-ui-images.sh --isolated-test-host --env-file PATH' >&2
  exit 2
fi
env_file=$3
cd "$(dirname "$0")/.."
namespace=${XELAB_M3_UI_NAMESPACE:-xelab-m3-ui}
if [[ ! "$namespace" =~ ^xelab-m3-ui(-[a-z0-9-]{1,32})?$ ]]; then
  echo 'invalid disposable test namespace' >&2
  exit 2
fi
case "$(uname -m)" in
  x86_64) platform=linux/amd64 ;;
  aarch64) platform=linux/arm64 ;;
  *) echo 'unsupported test host architecture' >&2; exit 2 ;;
esac
ctr_test() {
  if [[ $EUID -eq 0 ]]; then ctr -n "$namespace" "$@"; else sudo ctr -n "$namespace" "$@"; fi
}
private_dir=$(mktemp -d)
trap 'rm -rf -- "$private_dir"' EXIT
# This known key belongs only to disposable test images, matching the existing
# OCI test's fixed fixture publisher. It never enters a built image or report.
python3 - "$private_dir" <<'PY'
import os,pathlib,sys
path=pathlib.Path(sys.argv[1])/'publisher.der'
path.write_bytes(bytes.fromhex('302e020100300506032b657004220420')+bytes([7])*32)
os.chmod(path,0o600)
PY
openssl pkey -inform DER -in "$private_dir/publisher.der" -out "$private_dir/publisher.key"
publisher=00000000-0000-4000-8000-000000000001
python3 examples/webui/build_artifact.py --publisher-id "$publisher" --platform "$platform" --output examples/webui/generated-m3-ui
python3 examples/ui-reference/build_artifact.py --publisher-id "$publisher" --platform "$platform" --output examples/ui-reference/generated-m3-ui
for plugin in webui ui-reference; do
  tag="docker.io/xelab-ci/m3-$plugin:test"
  artifact="examples/$plugin/generated-m3-ui"
  tar_path="$artifact/image.tar"
  docker build --platform "$platform" -t "$tag" --build-arg "ARTIFACT_DIR=$artifact" -f "examples/$plugin/Dockerfile" .
  docker save "$tag" -o "$tar_path"
  ctr_test images import "$tar_path"
  # Delete exactly this successfully consumed tar. A failed import leaves its
  # input for diagnosis; unrelated image archives and tags are never removed.
  rm -- "$tar_path"
  record=$(ctr_test images list | awk -v image="$tag" '$1==image {print $2, $3}')
  read -r media digest <<< "$record"
  if [[ ! "$digest" =~ ^sha256:[a-f0-9]{64}$ || ( "$media" != 'application/vnd.oci.image.manifest.v1+json' && "$media" != 'application/vnd.docker.distribution.manifest.v2+json' ) ]]; then
    echo 'containerd did not report a platform-specific OCI manifest' >&2
    exit 1
  fi
  image="docker.io/xelab-ci/m3-$plugin@$digest"
  ctr_test images tag "$tag" "$image"
  python3 examples/echo/sign_artifact.py --manifest "$artifact/xelab/plugin.json" --image "$image" --key "$private_dir/publisher.key" > "$artifact/install.json"
  if [[ "$plugin" == webui ]]; then variable=XELAB_TEST_WEBUI_IMAGE; else variable=XELAB_TEST_UI_REFERENCE_IMAGE; fi
  python3 - "$env_file" "$variable" "$image" <<'PY'
import os,sys
fd=os.open(sys.argv[1],os.O_WRONLY|os.O_APPEND|os.O_CREAT|os.O_NOFOLLOW,0o600)
with os.fdopen(fd,'a') as output: output.write(sys.argv[2]+'='+sys.argv[3]+'\n')
PY
  echo "$variable=$image"
done
python3 - "$env_file" "$namespace" <<'PY'
import os,sys
fd=os.open(sys.argv[1],os.O_WRONLY|os.O_APPEND|os.O_CREAT|os.O_NOFOLLOW,0o600)
with os.fdopen(fd,'a') as output:
    output.write('XELAB_M3_UI_NAMESPACE='+sys.argv[2]+'\nXELAB_M3_UI_TEST=1\n')
PY
