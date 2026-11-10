#!/usr/bin/env python3
"""Prepare an isolated, root-owned real-daemon resource measurement.

Use only on the disposable CI measurement branch explicitly authorized to lower
the two NodeConfig reservation minima. This script does not change any schema,
start a daemon, relax root/TLS/authorization, or declare production acceptance.
No passwords, DSNs, or bearer values are printed or included in setup.json.
"""
import argparse
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import secrets
import shlex
import ssl
import stat
import subprocess
import sys
import time
import tomllib
import urllib.error
import urllib.request
import uuid


class SetupError(Exception):
    """Messages are fixed reason codes, never external output or secret data."""


def require(condition, code):
    if not condition:
        raise SetupError(code)


def private_write(path, data):
    if isinstance(data, str):
        data = data.encode()
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(data)


def private_read(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and stat.S_IMODE(info.st_mode) == 0o600
                and info.st_uid == 0 and info.st_nlink == 1, "unsafe_private_file")
        data = stream.read(65537)
    require(len(data) <= 65536, "private_file_too_large")
    return data.decode()


def run(command, label, *, data=None, env=None, timeout=60):
    result = subprocess.run(command, input=data, capture_output=True, env=env,
                            timeout=timeout, check=False)
    require(result.returncode == 0, label + "_failed")
    return result.stdout


def serialize_toml(value):
    lines = []
    for section, fields in value.items():
        require(isinstance(fields, dict), "unexpected_example_configuration")
        lines.append(f"[{section}]")
        for key, item in fields.items():
            require(isinstance(item, (str, int, bool, list)), "unsupported_toml_value")
            lines.append(f"{key} = {json.dumps(item, ensure_ascii=True)}")
        lines.append("")
    return "\n".join(lines)


def generate_certificates(root):
    openssl = ["openssl"]
    for prefix in ["server", "workload-ca"]:
        run(openssl + ["genpkey", "-algorithm", "EC", "-pkeyopt",
                       "ec_paramgen_curve:P-256", "-out", str(root / f"{prefix}.key")],
            "generate_" + prefix + "_key")
        os.chmod(root / f"{prefix}.key", 0o600)
    run(openssl + ["req", "-new", "-x509", "-sha256", "-days", "2",
                   "-key", str(root / "workload-ca.key"),
                   "-out", str(root / "workload-ca.crt"),
                   "-subj", "/CN=XeLab Resource Measurement Workload CA",
                   "-addext", "basicConstraints=critical,CA:TRUE",
                   "-addext", "keyUsage=critical,keyCertSign,cRLSign"], "workload_ca")
    # The test client explicitly trusts this exact self-signed service cert.
    # It remains a server leaf, never the authority for workload identities.
    run(openssl + ["req", "-new", "-x509", "-sha256", "-days", "2",
                   "-key", str(root / "server.key"), "-out", str(root / "server.crt"),
                   "-subj", "/CN=localhost",
                   "-addext", "basicConstraints=critical,CA:FALSE",
                   "-addext", "keyUsage=critical,digitalSignature",
                   "-addext", "extendedKeyUsage=serverAuth",
                   "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1"], "server_certificate")
    for name in ["server.crt", "workload-ca.crt"]:
        os.chmod(root / name, 0o600)
    # Verify the exact CA input used by Python/OpenSSL clients before startup.
    run(openssl + ["verify", "-CAfile", str(root / "server.crt"),
                   "-purpose", "sslserver", "-verify_hostname", "localhost",
                   str(root / "server.crt")], "server_hostname_verification")


def prepare(args):
    root, repo = args.root, args.repo
    require(not root.exists(), "measurement_root_must_be_fresh")
    root.mkdir(mode=0o700)
    config = tomllib.loads((repo / "docs/contracts/examples/kernel.toml").read_text())
    schema = json.loads((repo / "docs/contracts/openapi.json").read_text())
    properties = schema["components"]["schemas"]["NodeConfig"]["properties"]
    require(properties["kernel_cpu_millis"]["minimum"] <= 200
            and properties["kernel_memory_bytes"]["minimum"] <= 256 * 1024**2,
            "measurement_branch_floor_patch_required")
    generate_certificates(root)
    for name in ["token-hmac.key", "master.key"]:
        private_write(root / name, secrets.token_bytes(32))
    owner_password = secrets.token_urlsafe(36)
    private_write(root / "owner.password", owner_password)
    base_dsn = os.environ.get("XELAB_MEASURE_DATABASE_URL") or os.environ.get(
        "XELAB_TEST_DATABASE_URL")
    require(bool(base_dsn), "test_database_environment_required")
    require(not base_dsn.startswith(("postgresql:", "postgres:")),
            "libpq_keyword_test_dsn_required")
    database = "xelab_resource_" + uuid.uuid4().hex[:16]
    # PGDATABASE alone does not expand a keyword DSN. Map the deliberately
    # simple CI keyword DSN to libpq's individual environment fields, keeping
    # its password out of argv and all generated reports.
    pg_names = {"host": "PGHOST", "port": "PGPORT", "user": "PGUSER",
        "password": "PGPASSWORD", "dbname": "PGDATABASE", "sslmode": "PGSSLMODE",
        "connect_timeout": "PGCONNECT_TIMEOUT"}
    pg_env = {name: value for name, value in os.environ.items()
              if not name.startswith("PG")}
    seen = set()
    for item in shlex.split(base_dsn):
        name, separator, value = item.partition("=")
        require(separator and name in pg_names and name not in seen
                and "\n" not in value and "\r" not in value,
                "unsupported_test_database_keyword")
        pg_env[pg_names[name]] = value
        seen.add(name)
    require({"host", "user", "dbname"} <= seen, "explicit_test_database_fields_required")
    pg_env.setdefault("PGCONNECT_TIMEOUT", "5")
    run(["psql", "-X", "--no-password", "--set", "ON_ERROR_STOP=1", "--tuples-only",
         "--no-align", "--command", f'CREATE DATABASE "{database}"'],
        "create_isolated_database", env=pg_env)
    private_write(root / "database.dsn", base_dsn + " dbname=" + database)
    config["server"].update({
        "listen": "127.0.0.1:8443", "public_origin": "https://localhost:8443",
        "tls_cert_file": str(root / "server.crt"), "tls_key_file": str(root / "server.key"),
        "unix_socket": str(root / "control.sock"), "host_rpc_listen": "172.29.0.1:7443"})
    config["database"]["dsn_file"] = str(root / "database.dsn")
    config["runtime"].update({"socket": "/run/containerd/containerd.sock",
                              "namespace": "xelab-resource-measure",
                              "network_cidr": "172.29.0.0/24"})
    config["resources"].update({"kernel_cpu_millis": 200,
        "kernel_memory_bytes": 256 * 1024**2, "system_cpu_millis": 1000,
        "system_memory_bytes": 2 * 1024**3})
    config["storage"]["root_path"] = str(root / "state")
    config["storage"]["off_host_backup_uri"] = "s3://unused-resource-measurement/disabled/"
    config["security"].update({"token_hmac_key_file": str(root / "token-hmac.key"),
        "master_key_file": str(root / "master.key"),
        "workload_ca_cert_file": str(root / "workload-ca.crt"),
        "workload_ca_key_file": str(root / "workload-ca.key")})
    config.pop("ui_gateway", None)
    private_write(root / "kernel.toml", serialize_toml(config))
    executable = args.bin_dir / "xelabctl"
    run([str(executable), "validate-config", "--config", str(root / "kernel.toml")],
        "validate_configuration")
    # No DB metadata or resource budget is injected. The real migration and
    # bootstrap commands initialize a genuinely empty, independently named DB.
    migration = json.loads(run([str(executable), "migrate", "--config",
        str(root / "kernel.toml"), "--target", "19"], "migration"))
    require(migration.get("migration") == 19, "unexpected_migration_result")
    owner = json.loads(run([str(executable), "bootstrap", "--config",
        str(root / "kernel.toml"), "--owner-name", "Resource measurement owner"],
        "bootstrap", data=(owner_password + "\n").encode()))
    owner_id = str(uuid.UUID(owner["owner_id"]))
    ctr = ["ctr", "--address", config["runtime"]["socket"]]
    namespace = config["runtime"]["namespace"]
    namespaces = run(ctr + ["namespaces", "list", "--quiet"],
                     "observe_runtime_namespaces").decode().splitlines()
    if namespace not in namespaces:
        run(ctr + ["namespaces", "create", namespace], "create_runtime_namespace")
    else:
        containers = run(ctr + ["--namespace", namespace, "containers", "list", "--quiet"],
                         "observe_measurement_namespace")
        require(not containers.strip(), "measurement_namespace_is_not_empty")
    metadata = {"kind": "real daemon measurement setup", "owner_id": owner_id,
        "database_name": database, "root": str(root), "config": str(root / "kernel.toml"),
        "endpoint": config["server"]["public_origin"], "ca_file": str(root / "server.crt"),
        "resources": config["resources"], "namespace": config["runtime"]["namespace"],
        "production_acceptance": "pending issue #50; AWS DEFERRED",
        "measurement_only": True, "schema": 19}
    private_write(root / "setup.json", json.dumps(metadata, indent=2) + "\n")
    return metadata


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        return None


def operation(repo, name):
    document = json.loads((repo / "docs/contracts/openapi.json").read_text())
    matches = [(path, method.upper()) for path, methods in document["paths"].items()
               for method, definition in methods.items()
               if definition.get("operationId") == name]
    require(len(matches) == 1, "unknown_public_operation")
    return matches[0]


def login(args):
    root = args.root
    metadata = json.loads(private_read(root / "setup.json"))
    require(metadata["root"] == str(root), "setup_root_mismatch")
    config = tomllib.loads(private_read(root / "kernel.toml"))
    context = ssl.create_default_context(cafile=str(root / "server.crt"))
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}),
        urllib.request.HTTPSHandler(context=context), NoRedirect())
    origin = config["server"]["public_origin"]

    def api(name, *, body=None, token=None, parameters=None):
        path, method = operation(args.repo, name)
        for key, value in (parameters or {}).items():
            path = path.replace("{" + key + "}", str(uuid.UUID(value)))
        require("{" not in path, "missing_operation_parameter")
        headers = {}
        if token:
            headers["Authorization"] = "Bearer " + token
        payload = None
        if body is not None:
            headers["Content-Type"] = "application/json"
            headers["Idempotency-Key"] = str(uuid.uuid4())
            payload = json.dumps(body).encode()
        request = urllib.request.Request(origin + path, data=payload,
                                         headers=headers, method=method)
        try:
            with opener.open(request, timeout=5) as response:
                data, status = response.read(1048577), response.status
        except urllib.error.HTTPError as error:
            data, status = error.read(1048577), error.code
        require(len(data) <= 1048576, "public_response_too_large")
        return status, json.loads(data)

    deadline = time.monotonic() + 90
    while True:
        try:
            status, _ = api("getReadiness")
            if status == 200:
                break
        except (urllib.error.URLError, TimeoutError, ConnectionError):
            pass
        require(time.monotonic() < deadline, "real_daemon_readiness_timeout")
        time.sleep(1)
    status, issued = api("login", body={"account_id": metadata["owner_id"],
                                       "password": private_read(root / "owner.password")})
    require(status == 200 and isinstance(issued.get("access_token"), str),
            "owner_login_failed")
    owner_token = issued["access_token"]
    private_write(root / "owner.token", owner_token + "\n")
    expiry = (datetime.now(timezone.utc) + timedelta(hours=8)).isoformat().replace("+00:00", "Z")
    rule = {"action": "platform.read",
            "resource": {"kind": "platform", "match": "ids", "ids": ["node"]}}
    status, key = api("createApiKey", token=owner_token,
        parameters={"account_id": metadata["owner_id"]}, body={
            "name": "resource-measure-read", "inheritance": {"mode": "selected", "rules": [rule]},
            "conditions": {"expires_at": expiry, "max_calls": 100000}})
    require(status == 201 and isinstance(key.get("secret"), str), "read_key_creation_failed")
    private_write(root / "read.token", key["secret"] + "\n")
    status, _ = api("getNode", token=key["secret"])
    require(status == 200, "read_key_get_node_failed")
    # A harmless read requiring an unrelated action must remain unauthorized.
    denied, _ = api("getAccount", token=key["secret"],
                    parameters={"account_id": metadata["owner_id"]})
    require(denied == 403, "read_key_has_unexpected_account_access")
    metadata.update({"read_token_file": str(root / "read.token"),
        "owner_token_file": str(root / "owner.token"),
        "read_credential_id": str(uuid.UUID(key["credential"]["id"])),
        "read_rule": rule, "read_key_get_node_status": status,
        "read_key_unrelated_account_status": denied, "real_https_ready": True})
    # setup.json contains paths and IDs only, never bearer/password/DSN material.
    output = root / "setup.json"
    fd = os.open(output, os.O_WRONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, "w") as stream:
        require(os.fstat(stream.fileno()).st_uid == 0, "unsafe_setup_owner")
        stream.truncate(0)
        json.dump(metadata, stream, indent=2)
        stream.write("\n")
    (root / "owner.password").unlink()
    return metadata


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["prepare", "login"])
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--bin-dir", type=Path)
    args = parser.parse_args()
    require(os.geteuid() == 0, "root_required")
    require(args.repo.is_absolute() and args.root.is_absolute(), "absolute_paths_required")
    require(args.root.parent.is_dir() and not args.root.is_symlink(), "unsafe_measurement_root")
    args.repo = args.repo.resolve(strict=True)
    args.bin_dir = (args.bin_dir or args.repo / "target/release").resolve(strict=True)
    os.umask(0o077)
    result = prepare(args) if args.mode == "prepare" else login(args)
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except SetupError as error:
        print(json.dumps({"setup_failed": True, "reason": str(error)}), file=sys.stderr)
        raise SystemExit(1)
    except Exception as error:
        # External command/HTTP/DSN exception strings can contain secrets.
        print(json.dumps({"setup_failed": True, "error_type": type(error).__name__}), file=sys.stderr)
        raise SystemExit(1)
