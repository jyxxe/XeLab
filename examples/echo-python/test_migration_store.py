"""Real owned files; this is neither kernel admission nor OCI/XFS acceptance."""
import os
import pathlib
import shutil
import tempfile
import unittest
import uuid
from unittest.mock import patch

from migration_store import Store, atomic, read


class MigrationFiles(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.TemporaryDirectory()
        self.addCleanup(self.root.cleanup)
        root = pathlib.Path(self.root.name)
        self.original, self.data, self.scratch = [root / name for name in ("original", "data", "scratch")]
        for path in (self.original, self.data, self.scratch):
            path.mkdir(mode=0o700)
        self.source = Store(self.original, self.scratch, "1")
        self.source.initialize()
        self.source.append("one", "保留原数据 🌱")
        self.source.append("two", "ordered second record")
        shutil.copy2(self.original / "store.json", self.data / "store.json")
        self.store = Store(self.data, self.scratch, "2")
        self.deployment = str(uuid.uuid4())
        self.backup = str(uuid.uuid4())

    def plan(self):
        return self.store.plan("operation:plan", self.deployment, "1", "2")

    def test_read_only_plan_and_initialize_cannot_change_existing_format(self):
        before = (self.data / "store.json").read_bytes()
        os.chmod(self.data / "store.json", 0o400)
        os.chmod(self.data, 0o500)
        try:
            if os.geteuid() != 0:
                with self.assertRaises(PermissionError):
                    (self.data / "store.json").open("wb")
            plan = self.plan()
            self.assertEqual(self.plan(), plan)
            self.assertEqual((self.data / "store.json").read_bytes(), before)
            with self.assertRaisesRegex(ValueError, "cannot migrate"):
                self.store.initialize()
        finally:
            os.chmod(self.data, 0o700)
            os.chmod(self.data / "store.json", 0o600)
        self.assertEqual((self.data / "store.json").read_bytes(), before)

    def test_actual_process_exit_after_write_is_verified_without_a_second_write(self):
        plan_id, plan_digest, _ = self.plan()
        child = os.fork()
        if child == 0:
            try:
                self.store.apply(plan_id, plan_digest, self.backup, 12, after_write=lambda: os._exit(73))
            except BaseException:
                os._exit(75)
            os._exit(76)
        _, status = os.waitpid(child, 0)
        self.assertTrue(os.WIFEXITED(status))
        self.assertEqual(os.WEXITSTATUS(status), 73)
        reopened = Store(self.data, self.scratch, "2")
        self.assertEqual(reopened.verify(plan_id, 12), "2")
        self.assertEqual(reopened.check(), self.source.check())
        reopened.bind(plan_id, "operation:plan", self.deployment)
        for key, deployment in [("another:plan", self.deployment), ("operation:plan", str(uuid.uuid4()))]:
            with self.assertRaises(ValueError):
                reopened.bind(plan_id, key, deployment)

    def test_ambiguous_write_is_verified_from_new_process_state_without_apply_rewrite(self):
        original = (self.original / "store.json").read_bytes()
        plan_id, plan_digest, _ = self.plan()

        def lose_rpc():
            raise OSError("response lost after fsync and rename")

        with self.assertRaisesRegex(OSError, "response lost"):
            self.store.apply(plan_id, plan_digest, self.backup, 9, after_write=lose_rpc)
        reopened = Store(self.data, self.scratch, "2")
        self.assertEqual(read(self.data / "store.json")[1]["schema"], "2")
        self.assertEqual(reopened.verify(plan_id, 9), "2")
        with patch("migration_store.atomic", side_effect=AssertionError("must not rewrite an ambiguous Apply")):
            self.assertEqual(reopened.apply(plan_id, plan_digest, self.backup, 9), "2")
        self.assertEqual(reopened.check(), self.source.check())
        self.assertEqual((self.original / "store.json").read_bytes(), original)
        self.assertEqual(reopened.plan("operation:plan", self.deployment, "1", "2")[:2], (plan_id, plan_digest))
        with self.assertRaises(ValueError):
            reopened.apply(plan_id, plan_digest, str(uuid.uuid4()), 9)
        with self.assertRaises(ValueError):
            reopened.verify(plan_id, 10)

    def test_persisted_applying_without_a_committed_target_never_retries_write(self):
        plan_id, plan_digest, _ = self.plan()
        before = (self.data / "store.json").read_bytes()
        atomic(self.scratch / (plan_id + ".apply"), {"plan_digest": plan_digest, "backup_id": self.backup, "fence": 7})
        with self.assertRaises(ValueError):
            self.store.apply(plan_id, plan_digest, self.backup, 7)
        self.assertEqual((self.data / "store.json").read_bytes(), before)

    def test_changed_source_bad_binding_and_tampered_target_are_rejected(self):
        plan_id, plan_digest, _ = self.plan()
        with self.assertRaises(ValueError):
            self.store.apply(plan_id, "sha256:" + "0" * 64, self.backup, 7)
        for invalid in [0, -1, True, 2**53]:
            with self.assertRaises(ValueError):
                self.store.apply(plan_id, plan_digest, self.backup, invalid)
        Store(self.data, self.scratch, "1").append("new", "unfrozen write")
        before = (self.data / "store.json").read_bytes()
        with self.assertRaisesRegex(ValueError, "source data changed"):
            self.store.apply(plan_id, plan_digest, self.backup, 7)
        self.assertEqual((self.data / "store.json").read_bytes(), before)
        self.assertFalse((self.scratch / (plan_id + ".apply")).exists())
        shutil.copy2(self.original / "store.json", self.data / "store.json")
        self.store.apply(plan_id, plan_digest, self.backup, 7)
        _, target = read(self.data / "store.json")
        target["records"]["one"]["text"] = "corrupted"
        atomic(self.data / "store.json", target)
        with self.assertRaises(ValueError):
            self.store.verify(plan_id, 7)

    def test_plan_identity_binds_bytes_and_backup_restore_can_be_verified_read_only(self):
        plan_id, _, _ = self.plan()
        path = self.scratch / (plan_id + ".plan")
        _, value = read(path)
        value["count"] = 0
        atomic(path, value)
        with self.assertRaises(ValueError):
            self.store.verify(plan_id, 7)
        # A rollback restores a separately verified version 1 backup into a
        # fresh private target. The old program verifies it without downgrading
        # an existing version 2 store through Initialize or Apply.
        restored = Store(self.data, self.scratch, "1")
        restored_id, restored_digest, saved = restored.plan("rollback:plan", self.deployment, "2", "1")
        self.assertEqual(saved["kind"], "verify_restored")
        before = (self.data / "store.json").read_bytes()
        self.assertEqual(restored.verify(restored_id, 11), "1")
        with self.assertRaises(ValueError):
            restored.apply(restored_id, restored_digest, self.backup, 11)
        self.assertEqual((self.data / "store.json").read_bytes(), before)

    def test_empty_initialization_is_explicit_and_duplicate_or_linked_data_is_rejected(self):
        (self.data / "store.json").unlink()
        plan_id, _, saved = self.store.plan("bootstrap:plan", self.deployment, "none", "2")
        self.assertEqual(saved["kind"], "bootstrap")
        self.assertFalse((self.data / "store.json").exists())
        self.store.initialize()
        self.assertEqual(self.store.verify(plan_id, 3), "2")
        with self.assertRaises(ValueError):
            self.store.append("duplicate", "x" * 1025)
        (self.data / "store.json").write_bytes(b'{"schema":"2","schema":"2","records":{}}')
        with self.assertRaises(ValueError):
            self.store.check()
        (self.data / "store.json").unlink()
        os.symlink(self.original / "store.json", self.data / "store.json")
        with self.assertRaises(OSError):
            self.store.check()
        (self.data / "store.json").unlink()
        (self.data / "unknown").write_text("existing data")
        with self.assertRaises(ValueError):
            self.store.initialize()


if __name__ == "__main__":
    unittest.main()
