#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 1 ]]; then
  echo "Usage: bash tools/test-m3-ui.sh <cargo-test-artifacts.jsonl>" >&2
  exit 2
fi

test_binary=$(python3 tools/acceptance/cargo_executable.py "$1" test crates/kernel/tests/m3_ui.rs)
variables=XELAB_M3_UI_TEST,XELAB_M3_UI_NAMESPACE,XELAB_TEST_WEBUI_IMAGE,XELAB_TEST_UI_REFERENCE_IMAGE,XELAB_TEST_DATABASE_URL,XELAB_OAUTH_BROWSER_CHROMIUM,XELAB_OAUTH_BROWSER_PYTHON
sudo --preserve-env="$variables" "$test_binary" --ignored --nocapture
