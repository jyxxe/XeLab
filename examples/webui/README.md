# Official WebUI workload

This is the independent `org.xelab.webui` OCI application. The kernel retains authentication, authorization, routing and lifecycle control. This directory introduces no kernel dependency or management bypass.

The desktop uses browser ES modules and standard DOM/CSS without a frontend framework or package dependency. It includes a menu bar, Dock, searchable launcher, independent windows, pointer dragging, keyboard resizing/movement, minimize/maximize/restore/close/focus, narrow-screen layout and accessible labels. System/light/dark/custom themes and reduced motion/transparency use the current user's core UI preferences when that API is available. Unsaved settings are explicitly marked as previews.

Built-in windows issue actual public API requests for signed installations, deployments/approval/configuration/enabling, fixed Skill registration/state/invocation, accounts and child accounts, effective permission/grant sources, quotas/system status, UI clients, default UI and Operation lookup. JSON request editors retain their HTTP idempotency key on manual retry; a response is displayed as the actual core result, including an asynchronous Operation rather than invented completion. Errors, denied access, unavailable dependencies and empty results have separate states. Unimplemented Batch, Workflow, micro-plugin and knowledge-base capabilities remain visibly unavailable.

The browser performs authorization code + PKCE S256 through the exact configured trusted core origin. It never accepts platform passwords or reads the core login Cookie. Tokens and pending PKCE material are confined to this public client's tab session storage; refresh is single-flight, rotated credentials are not blindly retried, and logout clears private windows/state. The API module accepts only its closed operation map, exact parameters and the configured core origin. It is not an iframe HTTP bridge.

UI contributions can currently be discovered through the public core operation when available. Frame creation and the `xelab.ui/1` bridge remain an explicit next integration gate. The desktop does not substitute a same-origin iframe, arbitrary Skill picker or fake batch response.

See [deployment and signing](DEPLOYMENT.md) for the actual UID 65532 Lifecycle/static workload, resource configuration, callback path, certificate requirements, image build/signing and remaining core integration gates.

## Component verification

Run from the repository root:

```sh
node --test examples/webui/test_api.mjs
python3 -m unittest discover -s examples/webui -p 'test_server.py' -v
python3 examples/webui/test_browser.py
```

The browser suite requires Python Playwright and `/usr/bin/chromium`. It executes the actual desktop in Chromium with explicit core response fixtures. On 2026-11-04, 16 Node API/OAuth tests, 9 real loopback mTLS/static tests and 7 Chromium interaction tests passed without skips. The browser tests cover desktop/window controls, mobile control visibility, fixed Skill UUID requests and stable manual retry, text rendering of hostile names, denied/unavailable states, preference ETags/reload, logout clearing, trusted PKCE navigation, callback code/verifier exchange and wrong-state rejection. A native-browser fetch receiver error was found and fixed by these tests; Node mocks alone did not expose it.

The separate packaged Docker component probe described in the deployment guide also passed generated Lifecycle RPC and mTLS static checks. These results do **not** establish actual signed installation through the kernel, real Owner OAuth consent, core-to-OCI management, third-party iframe isolation, resource peak certification or complete M3-D/E acceptance. Those end-to-end tests require the HTTP/OAuth/WebUI core profiles and remain pending. AWS, independent-host recovery and formal RUN-03 are separately deferred or pending and are not covered by this application.
