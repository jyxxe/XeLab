//! Available-backup registration for a durable format transition. The caller
//! still owns runtime fencing, private destination paths and business Verify.
//! A Ready database label alone never constructs an authenticated point.
use super::*;
use crate::{
    backup_repository::Settings,
    contracts::digest,
    storage::migration_backup::{Expected, Point},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Persist with the original operation, then compare with current registration
/// before every use. Historical volume identities remain authorization inputs
/// even after a later upgrade publishes different volume UUIDs.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    expected: Expected,
    credential: Uuid,
    subject: Uuid,
    backup_operation: Uuid,
    backup_owner: Uuid,
    object_reference: String,
    master_key_id: String,
    volume_ids: Vec<Uuid>,
}

fn differs() -> Error {
    Error::conflict("Format backup no longer matches its available registration")
}

impl Registration {
    /// Runs before admission takes deployment/budget locks. The supplied
    /// installation is the source for an upgrade and the target for rollback.
    /// It must come from the parent's currently trusted installation check.
    pub async fn capture(
        c: &(impl GenericClient + Sync),
        app: &App,
        ctx: &Context,
        backup: Uuid,
        deployment: Uuid,
        installation: &Value,
    ) -> Result<Self> {
        let registration = Self::registered(c, ctx, backup, deployment, installation).await?;
        registration.authorize(c, ctx, None).await?;
        registration.point(app).await?;
        Ok(registration)
    }

    async fn registered(
        c: &(impl GenericClient + Sync),
        ctx: &Context,
        backup: Uuid,
        deployment: Uuid,
        installation: &Value,
    ) -> Result<Self> {
        let row = c.query_opt(
            "SELECT b.created_by,b.volume_ids,b.manifest_digest,b.object_reference,j.operation_id,j.repository_id,j.manifest_object_id,j.master_key_id,j.manifest,j.original_spec,j.installation_digest,j.installation_metadata FROM backups b JOIN storage_recovery_jobs j ON j.backup_id=b.id AND j.kind='backup' JOIN backup_repositories r ON r.id=j.repository_id WHERE b.id=$1 AND b.deployment_id=$2 AND j.deployment_id=$2 AND b.state='ready' AND b.off_host AND j.state='succeeded' AND r.state='ready'",
            &[&backup, &deployment],
        ).await?.ok_or_else(|| Error::conflict("Format migration requires an available off-host deployment backup"))?;
        let manifest = row
            .get::<_, Option<Value>>("manifest")
            .ok_or_else(differs)?;
        let manifest_digest = row
            .get::<_, Option<String>>("manifest_digest")
            .ok_or_else(differs)?;
        let object_reference = row
            .get::<_, Option<String>>("object_reference")
            .filter(|s| !s.is_empty() && s.len() <= 4096)
            .ok_or_else(differs)?;
        validate("Manifest", &installation["manifest"])?;
        artifact::validate_runtime_profile(&installation["manifest"])?;
        if super::installation(c, id(installation, "id")?).await? != *installation {
            return Err(differs());
        }
        validate("DeploymentSpec", &manifest["deployment_spec"])?;
        let registered: Value = row.get("installation_metadata");
        if serde_json::to_vec(installation)?.len() > 1048576
            || serde_json::to_vec(&manifest)?.len() > 1048576
            || manifest_digest != digest(&serde_json::to_vec(&manifest)?)
            || manifest["backup_id"] != backup.to_string()
            || manifest["deployment_id"] != deployment.to_string()
            || manifest["deployment_spec"] != row.get::<_, Value>("original_spec")
            || manifest["deployment_spec"]["installation_id"] != installation["id"]
            || manifest["master_key_reference"] != row.get::<_, String>("master_key_id")
            || row.get::<_, String>("installation_digest") != installation["image_digest"]
            || installation["publisher_id"] != installation["manifest"]["publisher_id"]
            || installation["manifest"]["data_schema_version"] == "none"
            || [
                "id",
                "plugin_id",
                "version",
                "image_digest",
                "manifest_digest",
                "manifest",
            ]
            .iter()
            .any(|field| {
                registered[*field] != installation[*field]
                    || manifest["installation"][*field] != installation[*field]
            })
        {
            return Err(differs());
        }
        let selected: Value = row.get("volume_ids");
        let selected = selected
            .as_array()
            .filter(|v| !v.is_empty() && v.len() <= 16)
            .ok_or_else(differs)?;
        let mut ids = BTreeSet::new();
        for value in selected {
            value
                .as_str()
                .and_then(|s| s.parse::<Uuid>().ok())
                .filter(|id| !id.is_nil() && ids.insert(*id))
                .ok_or_else(differs)?;
        }
        let manifest_selection = manifest["deployment_spec"]["volume_ids"]
            .as_array()
            .ok_or_else(differs)?;
        let historical: BTreeSet<Uuid> = manifest_selection
            .iter()
            .map(|v| v.as_str().and_then(|s| s.parse().ok()).ok_or_else(differs))
            .collect::<Result<_>>()?;
        if historical != ids || manifest_selection.len() != ids.len() {
            return Err(differs());
        }
        let registration = Self {
            expected: Expected {
                repository: row.get("repository_id"),
                backup,
                deployment,
                manifest_object: row.get("manifest_object_id"),
                manifest_digest,
                installation: installation.clone(),
                manifest,
            },
            credential: ctx.credential,
            subject: ctx.subject,
            backup_operation: row.get("operation_id"),
            backup_owner: row.get("created_by"),
            object_reference,
            master_key_id: row.get("master_key_id"),
            volume_ids: ids.into_iter().collect(),
        };
        if serde_json::to_vec(&registration)?.len() > 4 * 1048576 {
            return Err(Error::exhausted());
        }
        Ok(registration)
    }

    async fn authorize(
        &self,
        c: &(impl GenericClient + Sync),
        ctx: &Context,
        saved: Option<&[policy::Path]>,
    ) -> Result<()> {
        if self.credential != ctx.credential || self.subject != ctx.subject {
            return Err(Error::denied());
        }
        let current_owner: Uuid = c
            .query_opt(
                "SELECT created_by FROM backups WHERE id=$1",
                &[&self.expected.backup],
            )
            .await?
            .ok_or_else(Error::not_found)?
            .get(0);
        if current_owner != self.backup_owner {
            return Err(differs());
        }
        let rules = match saved {
            Some(paths) => Some(policy::rules(
                &Policy::new(c, ctx.peer)
                    .revalidate(paths, ctx.credential)
                    .await?,
            )),
            None => None,
        };
        let mut resources = vec![(
            "backup.read",
            Resource::new("backup", self.expected.backup, Some(self.backup_owner)),
        )];
        for volume in &self.volume_ids {
            // Tombstones retain owner_id. Physical availability is irrelevant
            // for archived source UUIDs; a missing or reassigned owner is not.
            let owner: Uuid = c
                .query_opt("SELECT owner_id FROM volumes WHERE id=$1", &[volume])
                .await?
                .ok_or_else(Error::not_found)?
                .get(0);
            if owner != ctx.subject {
                return Err(Error::denied());
            }
            resources.push(("volume.read", Resource::new("volume", volume, Some(owner))));
        }
        for (action, resource) in resources {
            match &rules {
                Some(rules) if !auth::rules_allow(rules, action, &resource, ctx.peer) => {
                    return Err(Error::denied())
                }
                Some(_) => {}
                None => auth::authorize(c, ctx, action, &resource).await?,
            }
        }
        Ok(())
    }

    /// Terminal replay checks the original scopes even when the backup has
    /// expired. Expiration prevents new I/O, not inspection of the old result.
    pub async fn authorize_replay(
        &self,
        c: &(impl GenericClient + Sync),
        ctx: &Context,
        original_authority: &[policy::Path],
    ) -> Result<()> {
        self.authorize(c, ctx, Some(original_authority)).await
    }

    /// Recheck original authority and current availability without remote I/O.
    /// The owning worker must hold its normal authorization fence for this call.
    pub async fn revalidate(
        &self,
        c: &(impl GenericClient + Sync),
        ctx: &Context,
        original_authority: &[policy::Path],
    ) -> Result<()> {
        self.authorize(c, ctx, Some(original_authority)).await?;
        let current = Self::registered(
            c,
            ctx,
            self.expected.backup,
            self.expected.deployment,
            &self.expected.installation,
        )
        .await?;
        if serde_json::to_value(current)? != serde_json::to_value(self)? {
            return Err(differs());
        }
        Ok(())
    }

    /// Rechecks original authority/current registration and obtains actual
    /// repository/manifest evidence. The owner still rechecks after filesystem
    /// work and before publication, under its authorization/runtime fences.
    pub async fn read(
        &self,
        c: &(impl GenericClient + Sync),
        app: &App,
        ctx: &Context,
        original_authority: &[policy::Path],
    ) -> Result<Point> {
        self.revalidate(c, ctx, original_authority).await?;
        self.point(app).await
    }

    async fn point(&self, app: &App) -> Result<Point> {
        let master = crate::backups::master(&app.config)?;
        if digest(master.as_ref()) != self.master_key_id {
            return Err(differs());
        }
        let repository = Settings::from_host(&app.config)?
            .bind(&app.db, self.expected.repository)
            .await?;
        if repository.objects.reference(&format!(
            "{}/{}.sealed",
            self.expected.backup, self.expected.manifest_object
        ))? != self.object_reference
        {
            return Err(differs());
        }
        let point = Point::read(repository, &master, &self.expected).await?;
        point.require_off_host()?;
        Ok(point)
    }

    pub fn backup(&self) -> Uuid {
        self.expected.backup
    }
    pub fn manifest_digest(&self) -> &str {
        &self.expected.manifest_digest
    }
}

#[cfg(test)]
#[path = "upgrades_backups_tests.rs"]
mod tests;
