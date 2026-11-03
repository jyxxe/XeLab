//! Authenticated format-backup reader. This component proves the expected
//! manifest/object bytes and a frozen clone or newly restored directory. The
//! owning job supplies authority, available-backup registration, protected
//! private paths, stopped writers and business DataMigration.Verify. In
//! particular, an authenticated schema label is not an on-disk format proof.
use super::backup_transfer::{Object, Pool, SavedUpload, Verified};
use crate::{
    backup_repository::Repository,
    contracts::{digest, id, validate},
    error::{Error, Result},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, path::PathBuf, time::Duration};
use uuid::Uuid;
use zeroize::Zeroizing;

/// Save this exact registration with the original durable migration intent.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    pub repository: Uuid,
    pub backup: Uuid,
    pub deployment: Uuid,
    pub manifest_object: Uuid,
    pub manifest_digest: String,
    pub installation: Value,
    pub manifest: Value,
}

struct Entry {
    source: Uuid,
    object: Uuid,
    spec: Value,
    saved: SavedUpload,
    capacity: u64,
}

/// Constructible only after actual bounded GET, AEAD and manifest validation.
/// Keep the repository object with this point; its identity cannot be swapped.
pub struct Point {
    repository: Repository,
    backup: Uuid,
    entries: Vec<Entry>,
}

fn invalid() -> Error {
    Error::conflict("Authenticated migration backup differs from its admitted registration")
}
fn sha(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}

impl Point {
    pub async fn read(
        repository: Repository,
        master: &[u8; 32],
        expected: &Expected,
    ) -> Result<Self> {
        if expected.repository.is_nil()
            || repository.id != expected.repository
            || expected.backup.is_nil()
            || expected.deployment.is_nil()
            || expected.manifest_object.is_nil()
            || !sha(&expected.manifest_digest)
            || serde_json::to_vec(&expected.manifest)?.len() > 1048576
        {
            return Err(invalid());
        }
        let manifest = repository
            .read_manifest(
                expected.backup,
                expected.manifest_object,
                master,
                &expected.manifest_digest,
            )
            .await?;
        let installation = &manifest["installation"];
        let wanted = &expected.installation;
        validate("Manifest", &wanted["manifest"])?;
        validate("DeploymentSpec", &manifest["deployment_spec"])?;
        let schema = wanted["manifest"]["data_schema_version"]
            .as_str()
            .filter(|s| {
                !s.is_empty() && *s != "none" && s.len() <= 64 && !s.chars().any(char::is_control)
            })
            .ok_or_else(invalid)?;
        if manifest != expected.manifest
            || manifest["format"] != 1
            || manifest["backup_id"] != expected.backup.to_string()
            || manifest["deployment_id"] != expected.deployment.to_string()
            || manifest["master_key_reference"] != digest(master)
            || manifest["deployment_spec"]["installation_id"] != wanted["id"]
            || wanted
                .get("publisher_id")
                .is_some_and(|publisher| publisher != &wanted["manifest"]["publisher_id"])
            || [
                "id",
                "image_digest",
                "manifest_digest",
                "plugin_id",
                "version",
                "manifest",
            ]
            .iter()
            .any(|field| installation[*field] != wanted[*field])
            || id(installation, "id")?.is_nil()
            || !sha(installation["image_digest"].as_str().unwrap_or(""))
            || !sha(installation["manifest_digest"].as_str().unwrap_or(""))
            || installation["plugin_id"] != installation["manifest"]["plugin_id"]
            || installation["version"] != installation["manifest"]["version"]
        {
            return Err(invalid());
        }
        let requirements = wanted["manifest"]["volumes"]
            .as_array()
            .filter(|v| !v.is_empty() && v.len() <= 16)
            .ok_or_else(invalid)?;
        let volumes = manifest["volumes"]
            .as_array()
            .filter(|v| v.len() == requirements.len())
            .ok_or_else(invalid)?;
        let mut sources = BTreeSet::new();
        let mut objects = BTreeSet::new();
        let mut aliases = BTreeSet::new();
        let mut entries = Vec::new();
        for volume in volumes {
            let source = id(volume, "source_volume_id")?;
            let object = id(volume, "object_id")?;
            let spec = &volume["volume_spec"];
            validate("VolumeSpec", spec)?;
            let capacity = spec["capacity_bytes"]
                .as_u64()
                .filter(|n| *n <= 63 * 1024 * 1024 * 1024)
                .ok_or_else(invalid)?;
            let alias = spec["name"].as_str().ok_or_else(invalid)?;
            let requirement = requirements
                .iter()
                .find(|r| r["name"] == alias)
                .ok_or_else(invalid)?;
            if source.is_nil()
                || object.is_nil()
                || !sources.insert(source)
                || !objects.insert(object)
                || !aliases.insert(alias.to_owned())
                || volume["capacity_bytes"] != capacity
                || volume["data_schema_version"] != schema
                || requirement["access"] != "rw"
                || requirement["scope"] != "deployment"
                || spec["scope"] != "deployment"
                || spec["access"] != "single_writer"
                || requirement["minimum_bytes"]
                    .as_u64()
                    .is_none_or(|n| n > capacity)
            {
                return Err(invalid());
            }
            let saved: SavedUpload =
                serde_json::from_value(volume["transfer"].clone()).map_err(|_| invalid())?;
            if saved.id.is_empty()
                || saved.id.len() > 4096
                || saved.etags.is_empty()
                || saved.etags.len() > 10000
                || saved
                    .etags
                    .iter()
                    .any(|s| s.is_empty() || s.len() > 256 || !s.is_ascii())
                || !sha(&saved.plaintext.digest)
                || !sha(&saved.ciphertext.digest)
                || saved.plaintext.bytes > capacity + capacity / 128 + 65 * 1024 * 1024
                || saved.ciphertext.bytes > 64 * 1024 * 1024 * 1024 + 64 * 1024 * 1024
                || saved.archive.entries == 0
                || saved.archive.entries > 65536
                || saved.archive.data_bytes > capacity
            {
                return Err(invalid());
            }
            entries.push(Entry {
                source,
                object,
                spec: spec.clone(),
                saved,
                capacity,
            });
        }
        let selection = manifest["deployment_spec"]["volume_ids"]
            .as_array()
            .ok_or_else(invalid)?;
        if selection.len() != sources.len() {
            return Err(invalid());
        }
        let selection: BTreeSet<Uuid> = selection
            .iter()
            .map(|v| v.as_str().and_then(|s| s.parse().ok()).ok_or_else(invalid))
            .collect::<Result<_>>()?;
        if selection != sources {
            return Err(invalid());
        }
        Ok(Self {
            repository,
            backup: expected.backup,
            entries,
        })
    }

    pub fn off_host(&self) -> bool {
        self.repository.objects.off_host()
    }

    /// Production migration admission MUST call this. Compatible test backends
    /// always keep their real false value; no caller-supplied flag promotes it.
    pub fn require_off_host(&self) -> Result<()> {
        if !self.off_host() {
            return Err(Error::unavailable(
                "Format migration requires a verified off-host backup",
            ));
        }
        Ok(())
    }

    pub fn volumes(&self) -> Vec<(Uuid, Value)> {
        self.entries
            .iter()
            .map(|v| (v.source, v.spec.clone()))
            .collect()
    }

    fn entry(&self, alias: &str) -> Result<&Entry> {
        self.entries
            .iter()
            .find(|v| v.spec["name"] == alias)
            .ok_or_else(invalid)
    }
    fn object(&self, entry: &Entry) -> Object {
        Object {
            backup: self.backup,
            object: entry.object,
            volume: entry.source,
            maximum_data: entry.capacity,
            maximum_archive: (entry.capacity + entry.capacity / 128 + 65 * 1024 * 1024)
                .min(64 * 1024 * 1024 * 1024),
        }
    }
    fn expected(&self, entry: &Entry) -> Verified {
        Verified {
            plaintext: entry.saved.plaintext.clone(),
            ciphertext: entry.saved.ciphertext.clone(),
            archive: entry.saved.archive.clone(),
            off_host: self.off_host(),
        }
    }

    /// Before Apply, compare the frozen private clone to the available backup.
    /// This rejects a stale point instead of migrating unbacked current writes.
    /// The archive verifier may restore atimes: never pass the public source.
    pub async fn verify_clone(
        &self,
        pool: &Pool,
        alias: &str,
        private_clone: PathBuf,
        master: Zeroizing<[u8; 32]>,
        timeout: Duration,
    ) -> Result<Verified> {
        let entry = self.entry(alias)?;
        pool.verify_restored(
            self.repository.objects.clone(),
            private_clone,
            self.object(entry),
            master,
            self.expected(entry),
            timeout,
        )?
        .wait()
        .await
    }

    /// Incompatible rollback restores to a newly reserved, empty private tree.
    /// The caller persists its restore checkpoint first and never retries an
    /// ambiguous extraction over existing files. Historical source UUIDs are
    /// archive identities; the new private allocation has a different UUID.
    pub async fn restore(
        &self,
        pool: &Pool,
        alias: &str,
        new_private: PathBuf,
        master: Zeroizing<[u8; 32]>,
        timeout: Duration,
    ) -> Result<Verified> {
        let entry = self.entry(alias)?;
        pool.restore(
            self.repository.objects.clone(),
            new_private,
            self.object(entry),
            master,
            self.expected(entry),
            timeout,
        )?
        .wait()
        .await
    }
}

#[cfg(test)]
#[path = "migration_backup_tests.rs"]
mod tests;
