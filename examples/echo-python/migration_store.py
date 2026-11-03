"""Bounded reference business format + durable opaque migration journal.

The kernel must freeze writers, supply a fresh private clone, mount business
data read-only during Plan, and reserve a separate writable scratch project.
This helper grants no kernel authority and does not publish volume metadata.
"""
import hashlib
import json
import os
import pathlib
import stat
import uuid

MAX_DATA = 1048576
# Two complete encodings (including worst-case JSON escapes) fit the reference
# manifest's 1 MiB minimum data quota during its atomic replacement.
MAX_RECORDS = 64
MAX_FENCE = 9007199254740991


def encoded(value):
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode()


def digest(raw):
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def unique(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("Duplicate JSON member")
        value[key] = item
    return value


def read(path):
    fd = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
    with os.fdopen(fd, "rb") as file:
        before = os.fstat(file.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or before.st_size > MAX_DATA:
            raise ValueError("Unsupported business/journal file")
        raw = file.read(MAX_DATA + 1)
        after = os.fstat(file.fileno())
        if len(raw) > MAX_DATA or (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
            raise ValueError("File changed or exceeded its bound")
    return raw, json.loads(raw, object_pairs_hook=unique)


def atomic(path, value, replace=True):
    raw = encoded(value)
    if len(raw) > MAX_DATA:
        raise ValueError("Business/journal file exceeds bound")
    temporary = path.with_name(".temporary-" + str(uuid.uuid4()))
    try:
        fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "wb") as file:
            file.write(raw)
            file.flush()
            os.fsync(file.fileno())
        if replace:
            os.replace(temporary, path)
        else:
            # An initialization or Plan replay must not overwrite an existing
            # record. Hard-link publication is atomic on this private project.
            os.link(temporary, path, follow_symlinks=False)
            temporary.unlink()
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if temporary.exists():
            temporary.unlink()


def records(value):
    if not isinstance(value, dict) or set(value) != {"schema", "items" if value.get("schema") == "1" else "records"}:
        raise ValueError("Unknown business format")
    if value["schema"] == "1":
        items = value["items"]
        if not isinstance(items, list) or len(items) > MAX_RECORDS:
            raise ValueError("Invalid version 1 records")
    elif value["schema"] == "2":
        entries = value["records"]
        if not isinstance(entries, dict) or len(entries) > MAX_RECORDS:
            raise ValueError("Invalid version 2 records")
        items = []
        order = set()
        for key, entry in entries.items():
            if (not isinstance(entry, dict) or set(entry) != {"text", "length", "ordinal"}
                    or not isinstance(entry["text"], str) or type(entry["length"]) is not int
                    or entry["length"] != len(entry["text"]) or type(entry["ordinal"]) is not int
                    or entry["ordinal"] in order or not 0 <= entry["ordinal"] < len(entries)):
                raise ValueError("Version 2 length/order verification failed")
            order.add(entry["ordinal"])
            items.append((entry["ordinal"], {"id": key, "text": entry["text"]}))
        items = [item for _, item in sorted(items)]
    else:
        raise ValueError("Unknown business schema")
    seen = set()
    for item in items:
        if (not isinstance(item, dict) or set(item) != {"id", "text"}
                or not isinstance(item["id"], str) or not 1 <= len(item["id"]) <= 128
                or item["id"] in seen or not isinstance(item["text"], str) or len(item["text"]) > 1024):
            raise ValueError("Invalid or duplicate business record")
        seen.add(item["id"])
    return items


def formatted(schema, items):
    if schema == "1":
        return {"schema": "1", "items": items}
    if schema == "2":
        return {"schema": "2", "records": {item["id"]: {"text": item["text"], "length": len(item["text"]), "ordinal": n} for n, item in enumerate(items)}}
    raise ValueError("Unsupported target schema")


class Store:
    def __init__(self, data, scratch, target):
        if target not in ("1", "2"):
            raise ValueError("Unsupported reference schema")
        self.data = pathlib.Path(data)
        self.scratch = pathlib.Path(scratch)
        self.target = target
        self.path = self.data / "store.json"

    def initialize(self):
        if not self.path.exists():
            if any(self.data.iterdir()):
                raise ValueError("Initialization requires empty business data")
            atomic(self.path, formatted(self.target, []), replace=False)
        self.check()

    def check(self):
        _, value = read(self.path)
        result = records(value)
        if value["schema"] != self.target:
            raise ValueError("Initialization cannot migrate an existing schema")
        return result

    def append(self, record_id, text):
        items = self.check()
        existing = next((item for item in items if item["id"] == record_id), None)
        if existing is not None:
            if existing["text"] != text:
                raise ValueError("Business idempotency key conflicts")
            return
        value = formatted(self.target, items + [{"id": record_id, "text": text}])
        records(value)
        atomic(self.path, value)

    def _plan_path(self, plan_id):
        value = uuid.UUID(plan_id)
        if value.int == 0 or str(value) != plan_id:
            raise ValueError("Invalid plan identity")
        return self.scratch / (plan_id + ".plan")

    def plan(self, operation_key, deployment, source, target):
        if not operation_key or len(operation_key) > 256 or target != self.target or source not in ("none", "1", "2"):
            raise ValueError("Invalid migration binding")
        deployment = str(uuid.UUID(deployment))
        operation_hash = digest(encoded({"deployment": deployment, "operation_key": operation_key}))
        index = self.scratch / (operation_hash[7:] + ".index")
        if index.exists():
            _, reference = read(index)
            plan_id = reference["plan_id"]
            saved = self._load(plan_id, reference["plan_digest"])
            if saved["deployment"] != deployment or saved["source"] != source or saved["target"] != target:
                raise ValueError("Plan replay binding differs")
            return plan_id, reference["plan_digest"], saved
        try:
            raw, value = read(self.path)
            items = records(value)
        except FileNotFoundError:
            if source != "none" or any(self.data.iterdir()):
                raise ValueError("Missing or nonempty business data")
            raw, value, items = b"", {"schema": "none"}, []
        if value["schema"] == target:
            kind = "verify_restored"
        elif source == "none" and value["schema"] == "none":
            kind = "bootstrap"
        elif source == value["schema"] == "1" and target == "2":
            kind = "upgrade"
        else:
            raise ValueError("MIGRATION_INCOMPATIBLE")
        expected = raw if kind == "verify_restored" else encoded(formatted(target, items))
        saved = {"deployment": deployment, "operation_hash": operation_hash, "source": source, "target": target, "kind": kind,
                 "source_digest": digest(raw), "target_digest": digest(expected), "count": len(items),
                 "temporary_bytes_required": 3 * MAX_DATA}
        plan_digest = digest(encoded(saved))
        plan_id = str(uuid.uuid5(uuid.NAMESPACE_URL, "xelab-reference-plan:" + plan_digest))
        path = self._plan_path(plan_id)
        if path.exists():
            self._load(plan_id, plan_digest)
        else:
            atomic(path, saved, replace=False)
        atomic(index, {"plan_id": plan_id, "plan_digest": plan_digest}, replace=False)
        return plan_id, plan_digest, saved

    def _load(self, plan_id, plan_digest=None):
        raw, saved = read(self._plan_path(plan_id))
        actual_digest = digest(raw)
        expected_id = str(uuid.uuid5(uuid.NAMESPACE_URL, "xelab-reference-plan:" + actual_digest))
        if (plan_digest is not None and actual_digest != plan_digest) or expected_id != plan_id or saved["target"] != self.target:
            raise ValueError("Plan digest or target differs")
        return saved

    def bind(self, plan_id, operation_key, deployment):
        saved = self._load(plan_id)
        expected = digest(encoded({"deployment": str(uuid.UUID(deployment)), "operation_key": operation_key}))
        if saved["operation_hash"] != expected:
            raise ValueError("Plan belongs to a different operation or deployment")

    @staticmethod
    def _fence(fence):
        if type(fence) is not int or not 1 <= fence <= MAX_FENCE:
            raise ValueError("Invalid migration write fence")

    def apply(self, plan_id, plan_digest, backup_id, fence, after_write=None):
        self._fence(fence)
        backup = uuid.UUID(backup_id)
        if backup.int == 0 or str(backup) != backup_id:
            raise ValueError("Invalid available backup identity")
        saved = self._load(plan_id, plan_digest)
        if saved["kind"] != "upgrade":
            raise ValueError("This plan only verifies initial/restored data")
        marker_path = self.scratch / (plan_id + ".apply")
        marker = {"plan_digest": plan_digest, "backup_id": backup_id, "fence": fence}
        if marker_path.exists():
            if read(marker_path)[1] != marker:
                raise ValueError("Apply binding differs")
            # An ambiguous old write is never repeated. Verify observes disk.
            return self.verify(plan_id, fence)
        raw, value = read(self.path)
        if digest(raw) != saved["source_digest"]:
            raise ValueError("Frozen source data changed")
        next_value = formatted(self.target, records(value))
        if digest(encoded(next_value)) != saved["target_digest"]:
            raise ValueError("Planned target differs")
        atomic(marker_path, marker, replace=False)
        atomic(self.path, next_value)
        if after_write is not None:
            after_write()  # Acceptance fault: disk committed, RPC outcome lost.
        return self.verify(plan_id, fence)

    def verify(self, plan_id, fence):
        self._fence(fence)
        saved = self._load(plan_id)
        marker_path = self.scratch / (plan_id + ".apply")
        if marker_path.exists() and read(marker_path)[1]["fence"] != fence:
            raise ValueError("Verify fence differs from the applying checkpoint")
        raw, value = read(self.path)
        items = records(value)
        if value["schema"] != self.target or digest(raw) != saved["target_digest"] or len(items) != saved["count"]:
            raise ValueError("Actual target data does not match the migration plan")
        return self.target
