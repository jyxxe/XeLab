# Independent WebUI OCI workload

This directory supplies the `org.xelab.webui` image service and public desktop assets. It uses no database connection, host mount, container runtime socket, user password, core login Cookie, or server-side management proxy. The browser calls the public core API as its registered OAuth client. The service accepts only the kernel workload identity on mTLS Lifecycle port 7000 and static HTTP port 8080.

The image runs as UID/GID 65532. Its declared minimum is 500 CPU millis, 536870912 memory bytes, 268435456 ephemeral bytes and 128 PIDs, charged to the ordinary plugin pool. It keeps at most 32 HTTP worker threads and an 8MiB image asset inventory. These configured bounds are not a measured peak resource certification and do not reduce the kernel's 4C16G reservation.

## Build and sign

Run commands from the repository root. Replace each example value with the actual registered publisher and isolated HTTPS shell origin. `--webui-origin` is part of the signed manifest; changing the origin requires a new artifact and normal approval.

```sh
python3 examples/webui/build_artifact.py \
  --publisher-id 00000000-0000-4000-8000-000000000001 \
  --webui-origin https://desktop.example.org
docker build -f examples/webui/Dockerfile -t docker.io/example/xelab-webui:1.0.0 .
docker save docker.io/example/xelab-webui:1.0.0 -o /tmp/xelab-webui-image.tar
sudo ctr -n xelab images import /tmp/xelab-webui-image.tar
sudo ctr -n xelab images list
```

Use the platform-specific manifest digest reported by containerd, rather than a mutable tag, for image registration and signing:

```sh
python3 examples/echo/sign_artifact.py \
  --manifest examples/webui/generated/xelab/plugin.json \
  --image docker.io/example/xelab-webui@sha256:REPLACE_WITH_ACTUAL_64_HEX_DIGEST \
  --key /secure/publisher-ed25519.pem
```

The signing helper rejects the placeholder digest. Keep the publisher private key outside the image and repository. Submit the actual signature through the ordinary publisher/installation APIs, approve the signed manifest, create a disabled deployment, and explicitly enable it with the existing management API or CLI. The normal lifecycle, admission, approval, image, identity and resource checks remain required; this example does not introduce an installation shortcut.

## Public configuration and OAuth registration

Register a public OAuth Client for the signed WebUI origin. Its exact redirect URI is:

```text
https://desktop.example.org/ui/org.xelab.webui/callback
```

Allowed origins must include the same exact canonical shell origin. Choose an authorized maximum rule ceiling for that client; registration does not grant the current user those rules. The browser uses authorization code plus PKCE S256 and requests only registered scope handles. No client secret is generated or deployed.

Set the deployment config to these public values, using the actual public client identifier and trusted core origin:

```json
{
  "core_origin": "https://core.example.org",
  "client_id": "xelab-REPLACE_WITH_REGISTERED_PUBLIC_ID",
  "management_api_major": 1,
  "default_page": "desktop",
  "default_theme": "system",
  "max_open_windows": 16,
  "max_active_widgets": 8,
  "widget_min_refresh_seconds": 30,
  "ui_protocol_major": 1
}
```

`core_origin` must be an exact canonical HTTPS origin and differ from the WebUI origin. Unknown configuration fields are rejected, including credentials. Initialize binds config to the expected deployment, instance, generation and SHA-256 digest. Repeating the same Initialize is idempotent; changing config or reviving a drained generation is refused.

Once initialized, the literal public module `/ui/org.xelab.webui/runtime-config.js` exports `runtimeConfig`. It contains only the above public configuration. All static GET/HEAD responses use no-store and expose no user data. Every undeclared path, query, encoded alternate spelling, Range request, credential header, Cookie, WebSocket upgrade and write method is refused by the workload.

The public gateway must separately implement the approved WebUi profile: resolve the exact registered callback, strip its OAuth query before forwarding to this static workload, retain the query in the browser location, and apply a shell CSP that permits only the trusted core API as a connection target. The ordinary contribution-page CSP remains `connect-src 'none'`. Do not make a generic query or origin forwarding exception. The service intentionally supplies no permissive CSP that could override the gateway's trusted policy.

The browser entry is `/ui/org.xelab.webui/`; the callback is a literal alias for the same image-baked HTML entry. The core's unavailable-WebUI recovery path and CLI remain independent of this service.

## Local component checks and remaining integration

```sh
python3 -m unittest discover -s examples/webui -p 'test_server.py' -v
```

These tests create their own short-lived CA and connect to a real loopback mTLS server. They cover literal asset/HEAD/conditional responses, callback and traversal/query rejection, strict public configuration, absent/foreign workload identities, credential/protocol header rejection, and lifecycle admission/drain state. They do not run the kernel, an OCI container, the generated Lifecycle RPC, browser OAuth, or plugin contribution isolation.

The separately requested component image runs the actual packaged server entry point as UID 65532, using generated protobuf clients and a disposable test CA. It checks Lifecycle Describe/Initialize/digest/generation rejection, static readiness/config/callback, Drain, Shutdown, foreign RPC identity and missing client certificate. It requires a Docker-capable environment and the artifact from the build step:

```sh
docker build --target component-tests -f examples/webui/Dockerfile -t xelab-webui-component .
docker run --rm --read-only --tmpfs /tmp:rw,nosuid,nodev,size=32m xelab-webui-component
```

This optional entry point is `test_server.py --rpc-probe`; its successful report explicitly records `kernel_oci_integration=not_executed`. It was executed locally on 2026-11-04 with component image `sha256:fea3d7fe1df600f505c185fe8b5dc398cdba163f31169f411c88a2213388ea4e`, UID 65532, a read-only root and a 32MiB `/tmp`, and passed the checks above. The first probe attempt exposed an unreadable copied test file; the component target now explicitly copies it with mode 0444. The 9 default server tests also passed without skipped cases.

This component run does not prove the kernel's actual signed installation path. The production image remains the final default `webui` target and does not contain the test probe.

The default production target was also built locally as `sha256:5af52fb1965510d434bd6a417c6e7399ecef191d4e46bbdf37481eefc073fc2f`. A read-only container check verified UID 65532, readable desktop assets and absence of the test probe. These local image identifiers are execution evidence, not published or signed release references.

The current integration gates remain explicit: the kernel must admit the approved WebUi profile and client/origin binding; its OAuth/callback/CSP policy must be active; the image must pass actual signed OCI install/Initialize/health/disable/restart tests; the browser must complete real Owner OAuth and authorized API calls; and third-party frames require their own isolation/message-bridge acceptance. Do not treat a static build or these loopback tests as M3-D/E completion. AWS, independent-host and RUN-03 acceptance are outside these component checks.
