#!/usr/bin/env bash
# Owned loopback fixture only. The CI runner cleans up its tracked process at
# job exit. No existing service, bucket, credential file or data is adopted.
set -euo pipefail
umask 077
fixture_root=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/xelab-s3.XXXXXXXX")
go_version=1.24.13
go_sha=1fc94b57134d51669c72173ad5d49fd62afb0f1db9bf3f798fd98ee423f8d730
minio_commit=0d7408fc9969caf07de6a8c3a84f9fbb10a6739e
test "$(uname -m)" = x86_64
curl --proto '=https' --tlsv1.2 --fail --location --retry 3 --max-time 180 \
  "https://go.dev/dl/go${go_version}.linux-amd64.tar.gz" -o "$fixture_root/go.tar.gz"
echo "$go_sha  $fixture_root/go.tar.gz" | sha256sum --check --status
tar -xzf "$fixture_root/go.tar.gz" -C "$fixture_root"
git init -q "$fixture_root/source"
git -C "$fixture_root/source" remote add origin https://github.com/minio/minio.git
git -C "$fixture_root/source" fetch -q --depth 1 origin "$minio_commit"
git -C "$fixture_root/source" checkout -q --detach FETCH_HEAD
test "$(git -C "$fixture_root/source" rev-parse HEAD)" = "$minio_commit"
(
  cd "$fixture_root/source"
  GOTOOLCHAIN=local GOMAXPROCS=2 GOMEMLIMIT=1GiB CGO_ENABLED=0 \
    "$fixture_root/go/bin/go" build -p 2 -trimpath -o "$fixture_root/minio" .
)
python3 - "$fixture_root" <<'PY'
import os, pathlib, socket, subprocess, sys, time, urllib.request
root = pathlib.Path(sys.argv[1])
with socket.socket() as probe:
    probe.bind(("127.0.0.1", 9000))
env = {k: v for k, v in os.environ.items() if k in (
    "PATH", "HOME", "LANG", "SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "RUNNER_TRACKING_ID"
)}
env.update(MINIO_ROOT_USER="xelab-test", MINIO_ROOT_PASSWORD="test-only-backup-password",
           MINIO_BROWSER="off", MINIO_UPDATE="off", GOMAXPROCS="1", GOMEMLIMIT="768MiB")
with (root / "server.log").open("wb") as log:
    process = subprocess.Popen([str(root / "minio"), "server", str(root / "data"),
                                "--address", "127.0.0.1:9000"],
                               env=env, stdin=subprocess.DEVNULL, stdout=log,
                               stderr=subprocess.STDOUT, start_new_session=True)
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
for _ in range(120):
    if process.poll() is not None:
        raise SystemExit("Owned S3 fixture exited before readiness")
    try:
        with opener.open("http://127.0.0.1:9000/minio/health/ready", timeout=1) as response:
            if response.status == 200:
                break
    except OSError:
        pass
    time.sleep(0.25)
else:
    process.terminate()
    raise SystemExit("Owned S3 fixture did not become ready")
if os.environ.get("GITHUB_ENV"):
    with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as output:
        output.write("XELAB_TEST_S3_ENDPOINT=http://127.0.0.1:9000\n")
        output.write(f"XELAB_TEST_S3_FIXTURE={root}\n")
print("Owned S3 fixture ready on http://127.0.0.1:9000")
PY
