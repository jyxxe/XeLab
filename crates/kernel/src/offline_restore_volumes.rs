//! Offline-only rebinding: logical resources survive, physical roots/project
//! IDs do not. No filesystem work is performed inside the reservation transaction.
use super::*;
use crate::storage::xfs::{Limits, ProjectStatus, ProjectStore};
use serde_json::json;
use std::time::Duration;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Item {
    pub volume: Uuid,
    pub source_project: i32,
    pub project: i32,
    pub owner: Uuid,
    pub spec: Value,
    pub capacity: u64,
    pub inodes: u64,
    pub metadata: u64,
}
impl Item {
    fn limits(&self) -> Limits {
        Limits {
            capacity_bytes: self.capacity,
            inodes: self.inodes,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Plan {
    pub high_water: i64,
    pub source_root: Uuid,
    pub source_filesystem: Uuid,
    pub root: Uuid,
    pub filesystem: Uuid,
    pub capacity: u64,
    pub reserved: u64,
    pub items: Vec<Item>,
}
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| Error::unavailable("Offline volume worker stopped"))?
}
fn workspace(target: &Target) -> Result<&Workspace> {
    target
        .workspace
        .as_ref()
        .ok_or_else(|| Error::conflict("Recovery workspace is unavailable"))
}
async fn make_plan(target: &Target) -> Result<Option<Plan>> {
    let c = target.session.db.connect().await?;
    let root = c
        .query_opt("SELECT id,filesystem_uuid FROM volume_storage_root", &[])
        .await?;
    let rows=c.query("SELECT a.*,v.spec AS public_spec,v.owner_id AS public_owner FROM volume_allocations a LEFT JOIN volumes v ON v.id=a.volume_id WHERE a.state<>'deleted' ORDER BY a.volume_id LIMIT 17",&[]).await?;
    let live: i64 = c
        .query_one("SELECT count(*) FROM volumes WHERE state<>'deleted'", &[])
        .await?
        .get(0);
    if rows.len() != target.manifest.volumes.len() || live != rows.len() as i64 || rows.len() > 16 {
        return Err(Error::conflict(
            "Imported volume set differs from the platform snapshot",
        ));
    }
    let Some(root) = root else {
        if !rows.is_empty() {
            return Err(Error::conflict(
                "Imported volumes have no physical source binding",
            ));
        }
        return Ok(None);
    };
    let source_root: Uuid = root.get("id");
    let source_filesystem: Uuid = root.get("filesystem_uuid");
    let high_water: i64 = c
        .query_one("SELECT last_value FROM volume_project_ids", &[])
        .await?
        .get(0);
    let mut items = Vec::new();
    for row in rows {
        let id: Uuid = row.get("volume_id");
        let v = target
            .manifest
            .volumes
            .iter()
            .find(|v| v.object.volume == id)
            .ok_or_else(|| Error::conflict("Unexpected imported volume"))?;
        let owner: Uuid = row.get("owner_id");
        let spec: Value = row.get("spec");
        let source_project: i32 = row.get("project_id");
        if row.get::<_, String>("state") != "ready"
            || row.get::<_, Uuid>("root_id") != source_root
            || source_project <= 0
            || v.source_project_id != source_project as u32
            || v.filesystem_uuid != source_filesystem
            || v.spec != spec
            || row.get::<_, Option<Value>>("public_spec") != Some(spec.clone())
            || row.get::<_, Option<Uuid>>("public_owner") != Some(owner)
            || row.get::<_, i64>("capacity_bytes") as u64 != v.object.maximum_data
        {
            return Err(Error::conflict("Imported volume source plan differs"));
        }
        let project: i64 = c
            .query_one("SELECT nextval('volume_project_ids')", &[])
            .await?
            .get(0);
        let project = i32::try_from(project).map_err(|_| Error::exhausted())?;
        if project <= source_project {
            return Err(Error::conflict(
                "Recovery projects must advance the source high water mark",
            ));
        }
        items.push(Item {
            volume: id,
            source_project,
            project,
            owner,
            spec,
            capacity: v.object.maximum_data,
            inodes: 0,
            metadata: 0,
        });
    }
    let tombstones: i64 = c
        .query_one(
            "SELECT count(*) FROM volume_allocations WHERE state='deleted'",
            &[],
        )
        .await?
        .get(0);
    let path = target.session.runtime.config.root().join("volumes");
    let plan = blocking(move || {
        let store = ProjectStore::open(&path)?;
        store.verify_mount_root()?;
        let status = store.status()?;
        if status.filesystem_uuid == source_filesystem {
            return Err(Error::conflict(
                "Recovery requires a new volume filesystem UUID",
            ));
        }
        let capacity = store
            .available_bytes()?
            .saturating_sub(64 * 1024 * 1024)
            .min(9_007_199_254_740_991)
            / status.block_size
            * status.block_size;
        let mut reserved = (tombstones as u64)
            .checked_mul(65536)
            .ok_or_else(Error::exhausted)?;
        let mut items = items;
        for item in &mut items {
            let limits = store.aligned_limits(item.capacity)?;
            if limits.capacity_bytes != item.capacity {
                return Err(Error::conflict(
                    "Target volume geometry cannot preserve the original capacity",
                ));
            }
            item.inodes = limits.inodes;
            item.metadata = store.metadata_charge(limits)?;
            reserved = reserved
                .checked_add(item.capacity)
                .and_then(|n| n.checked_add(item.metadata))
                .ok_or_else(Error::exhausted)?;
        }
        if capacity < 1048576 || reserved > capacity {
            return Err(Error::exhausted());
        }
        Ok(Plan {
            high_water,
            source_root,
            source_filesystem,
            root: Uuid::new_v4(),
            filesystem: status.filesystem_uuid,
            capacity,
            reserved,
            items,
        })
    })
    .await?;
    Ok(Some(plan))
}
fn check_plan(target: &Target, plan: &Plan) -> Result<()> {
    let mut ids = BTreeSet::new();
    let mut projects = BTreeSet::new();
    let mut minimum = 0u64;
    if plan.source_root.is_nil()
        || plan.high_water < 1
        || plan.high_water > i32::MAX as i64
        || plan.root.is_nil()
        || plan.root == plan.source_root
        || plan.source_filesystem.is_nil()
        || plan.filesystem.is_nil()
        || plan.filesystem == plan.source_filesystem
        || plan.capacity < 1048576
        || plan.reserved > plan.capacity
        || plan.capacity > 9_007_199_254_740_991
        || plan.items.len() != target.manifest.volumes.len()
    {
        return Err(Error::conflict("Invalid recovery volume binding"));
    }
    for item in &plan.items {
        let source = target
            .manifest
            .volumes
            .iter()
            .find(|v| v.object.volume == item.volume)
            .ok_or_else(|| Error::conflict("Unexpected recovery volume"))?;
        if !ids.insert(item.volume)
            || item.source_project <= 0
            || item.project <= item.source_project
            || i64::from(item.project) <= plan.high_water
            || !projects.insert(item.project)
            || item.source_project as u32 != source.source_project_id
            || source.filesystem_uuid != plan.source_filesystem
            || item.owner.is_nil()
            || item.spec != source.spec
            || item.capacity != source.object.maximum_data
            || item.inodes == 0
            || item.inodes > 9_007_199_254_740_991
            || item.metadata < 65536
            || item.metadata > 9_007_199_254_740_991
        {
            return Err(Error::conflict("Invalid recovery volume reservation"));
        }
        minimum = minimum
            .checked_add(item.capacity)
            .and_then(|n| n.checked_add(item.metadata))
            .ok_or_else(Error::exhausted)?;
    }
    if plan.reserved < minimum {
        return Err(Error::conflict(
            "Recovery volume budget omits admitted capacity",
        ));
    }
    Ok(())
}
async fn reserve(target: &Target, plan: &Plan) -> Result<()> {
    check_plan(target, plan)?;
    target.session.check_leader()?;
    let mut c = target.session.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, true).await?;
    let tombstones: i64 = tx
        .query_one(
            "SELECT count(*) FROM volume_allocations WHERE state='deleted'",
            &[],
        )
        .await?
        .get(0);
    let expected = plan.items.iter().try_fold(
        (tombstones as u64)
            .checked_mul(65536)
            .ok_or_else(Error::exhausted)?,
        |n, item| {
            n.checked_add(item.capacity)
                .and_then(|n| n.checked_add(item.metadata))
                .ok_or_else(Error::exhausted)
        },
    )?;
    if expected != plan.reserved {
        return Err(Error::conflict(
            "Recovery reservation total differs from its complete allocation set",
        ));
    }
    if tx.query_opt("SELECT recovery_id FROM platform_recovery_state WHERE recovery_id=$1 AND backup_id=$2 AND phase='rebuilding' FOR UPDATE",&[&target.recovery,&target.receipt.backup_id]).await?.is_none() {return Err(Error::conflict("Recovery volume gate differs"));}
    let row = tx
        .query_one(
            "SELECT id,filesystem_uuid,capacity_bytes,reserved_bytes,state FROM volume_storage_root FOR UPDATE",
            &[],
        )
        .await?;
    let current: Uuid = row.get("id");
    if current == plan.root {
        if row.get::<_, Uuid>("filesystem_uuid") != plan.filesystem
            || row.get::<_, i64>("capacity_bytes") != plan.capacity as i64
            || row.get::<_, i64>("reserved_bytes") != plan.reserved as i64
            || row.get::<_, String>("state") != "preparing"
        {
            return Err(Error::conflict("Recovery filesystem binding differs"));
        }
    } else {
        if current != plan.source_root
            || row.get::<_, Uuid>("filesystem_uuid") != plan.source_filesystem
        {
            return Err(Error::conflict("Imported source volume root differs"));
        }
        // This named FK is part of the trusted v9 schema. Restore its original
        // immediate/non-deferrable behavior inside this same transaction.
        tx.batch_execute("ALTER TABLE volume_allocations ALTER CONSTRAINT volume_allocations_root_id_fkey DEFERRABLE INITIALLY DEFERRED; SET CONSTRAINTS volume_allocations_root_id_fkey DEFERRED").await?;
        tx.execute("UPDATE volume_storage_root SET id=$1,filesystem_uuid=$2,state='preparing',capacity_bytes=$3,reserved_bytes=$4 WHERE id=$5",&[&plan.root,&plan.filesystem,&(plan.capacity as i64),&(plan.reserved as i64),&plan.source_root]).await?;
        tx.execute(
            "UPDATE volume_allocations SET root_id=$1 WHERE root_id=$2",
            &[&plan.root, &plan.source_root],
        )
        .await?;
        for item in &plan.items {
            let updated=tx.execute("UPDATE volume_allocations SET project_id=$2,capacity_bytes=$3,inode_limit=$4,metadata_charge=$5,desired_capacity=$3,desired_inodes=$4,desired_metadata_charge=$5,state='restoring',updated_at=now() WHERE volume_id=$1 AND root_id=$6 AND project_id=$7 AND state='ready' AND owner_id=$8 AND spec=$9",&[&item.volume,&item.project,&(item.capacity as i64),&(item.inodes as i64),&(item.metadata as i64),&plan.root,&item.source_project,&item.owner,&item.spec]).await?;
            if updated != 1 {
                return Err(Error::conflict("Recovery volume allocation source changed"));
            }
            tx.execute("UPDATE volumes SET writer_instance_id=NULL,revision=revision+1,updated_at=now() WHERE id=$1",&[&item.volume]).await?;
        }
        tx.batch_execute("SET CONSTRAINTS volume_allocations_root_id_fkey IMMEDIATE; ALTER TABLE volume_allocations ALTER CONSTRAINT volume_allocations_root_id_fkey NOT DEFERRABLE INITIALLY IMMEDIATE").await?;
    }
    for item in &plan.items {
        if tx.query_opt("SELECT volume_id FROM volume_allocations WHERE volume_id=$1 AND root_id=$2 AND project_id=$3 AND owner_id=$4 AND spec=$5 AND capacity_bytes=$6 AND inode_limit=$7 AND metadata_charge=$8 AND state IN ('restoring','restored')",&[&item.volume,&plan.root,&item.project,&item.owner,&item.spec,&(item.capacity as i64),&(item.inodes as i64),&(item.metadata as i64)]).await?.is_none() {return Err(Error::conflict("Recovery reservation checkpoint differs"));}
    }
    target.session.check_leader()?;
    tx.commit().await?;
    Ok(())
}
pub(super) async fn restore(target: &mut Target) -> Result<()> {
    let ws = workspace(target)?;
    let header = ws.header()?;
    let plan = if let Some(value) = header.context.get("volume_plan") {
        serde_json::from_value::<Option<Plan>>(value.clone())?
    } else {
        let plan = make_plan(target).await?;
        let mut context = header.context;
        context["volume_plan"] = serde_json::to_value(&plan)?;
        ws.update_header(&header.phase, context)?;
        plan
    };
    if let Some(plan) = plan {
        reserve(target, &plan).await?;
        let path = target.session.runtime.config.root().join("volumes");
        let identity = plan.clone();
        blocking(move || {
            let mut store = ProjectStore::open(&path)?;
            if store.status()?.filesystem_uuid != identity.filesystem {
                return Err(Error::conflict("Recovery volume filesystem changed"));
            }
            for item in &identity.items {
                let limits = store.aligned_limits(item.capacity)?;
                if limits != item.limits() || store.metadata_charge(limits)? != item.metadata {
                    return Err(Error::conflict("Recovery volume geometry changed"));
                }
            }
            store.namespace_identity(identity.filesystem, identity.root, true)
        })
        .await?;
        let pool = target.pool.clone();
        for item in &plan.items {
            target.session.check_leader()?;
            let c = target.session.db.connect().await?;
            let state:String=c.query_one("SELECT state FROM volume_allocations WHERE volume_id=$1 AND root_id=$2 AND project_id=$3",&[&item.volume,&plan.root,&item.project]).await?.get(0);
            let path = target.session.runtime.config.root().join("volumes");
            let planned = item.clone();
            let root = plan.clone();
            let data = blocking(move || {
                let mut store = ProjectStore::open(&path)?;
                store.namespace_identity(root.filesystem, root.root, false)?;
                store.recover_create(planned.volume, planned.project as u32, planned.limits())?;
                store.prepare_data(planned.volume, planned.project as u32)?;
                store.data_path(planned.volume, planned.project as u32)
            })
            .await?;
            let source = target
                .manifest
                .volumes
                .iter()
                .find(|v| v.object.volume == item.volume)
                .unwrap();
            let expected = source
                .verified
                .clone()
                .ok_or_else(|| Error::conflict("Missing authenticated volume proof"))?;
            // Existing complete targets are verified, never unpacked over.
            let verified = pool
                .verify_restored(
                    target.repository.objects.clone(),
                    data.clone(),
                    source.object,
                    zeroize::Zeroizing::new(*target.master),
                    expected.clone(),
                    Duration::from_secs(3600),
                )?
                .wait()
                .await;
            if let Err(error) = verified {
                if state == "restored" {
                    return Err(error);
                }
                let path = target.session.runtime.config.root().join("volumes");
                let planned = item.clone();
                let root = plan.clone();
                let data = blocking(move || {
                    let mut store = ProjectStore::open(&path)?;
                    store.namespace_identity(root.filesystem, root.root, false)?;
                    store.clear_private_data(planned.volume, planned.project as u32)?;
                    store.prepare_data(planned.volume, planned.project as u32)?;
                    store.data_path(planned.volume, planned.project as u32)
                })
                .await?;
                pool.restore(
                    target.repository.objects.clone(),
                    data,
                    source.object,
                    zeroize::Zeroizing::new(*target.master),
                    expected,
                    Duration::from_secs(3600),
                )?
                .wait()
                .await?;
            }
            target.session.check_leader()?;
            let c = target.session.db.connect().await?;
            c.execute("UPDATE volume_allocations SET state='restored',updated_at=now() WHERE volume_id=$1 AND root_id=$2 AND project_id=$3 AND state IN ('restoring','restored')",&[&item.volume,&plan.root,&item.project]).await?;
        }
    }
    target.session.check_leader()?;
    let mut header = ws.header()?;
    header.context["volumes_verified"] = json!(true);
    ws.update_header(&header.phase, header.context)
}
pub(super) struct Publication {
    plan: Option<Plan>,
    status: Vec<(Uuid, ProjectStatus)>,
}
pub(super) async fn verify(target: &Target) -> Result<Publication> {
    let header = workspace(target)?.header()?;
    if header.context["volumes_verified"] != true {
        return Err(Error::conflict("Volume restoration is incomplete"));
    }
    let plan: Option<Plan> = serde_json::from_value(header.context["volume_plan"].clone())?;
    let mut status = Vec::new();
    if let Some(plan) = &plan {
        check_plan(target, plan)?;
        for item in &plan.items {
            target.session.check_leader()?;
            let path = target.session.runtime.config.root().join("volumes");
            let volume = item.volume;
            let item = item.clone();
            let plan = plan.clone();
            let (data, observed) = blocking(move || {
                let mut store = ProjectStore::open(&path)?;
                store.namespace_identity(plan.filesystem, plan.root, false)?;
                let observed = store.inspect(item.volume, item.project as u32)?;
                if observed.limits != item.limits()
                    || observed.used_bytes > item.capacity
                    || observed.used_inodes > item.inodes
                {
                    return Err(Error::conflict(
                        "Restored project usage or hard limits differ",
                    ));
                }
                Ok((store.data_path(item.volume, item.project as u32)?, observed))
            })
            .await?;
            let source = target
                .manifest
                .volumes
                .iter()
                .find(|v| v.object.volume == volume)
                .unwrap();
            target
                .pool
                .verify_restored(
                    target.repository.objects.clone(),
                    data,
                    source.object,
                    zeroize::Zeroizing::new(*target.master),
                    source
                        .verified
                        .clone()
                        .ok_or_else(|| Error::conflict("Missing volume proof"))?,
                    Duration::from_secs(3600),
                )?
                .wait()
                .await?;
            status.push((source.object.volume, observed));
        }
    }
    Ok(Publication { plan, status })
}
pub(super) async fn publish(
    tx: &tokio_postgres::Transaction<'_>,
    proof: Publication,
) -> Result<()> {
    if let Some(plan)=proof.plan {
        let row=tx.query_one("SELECT id,filesystem_uuid,capacity_bytes,reserved_bytes,state FROM volume_storage_root FOR UPDATE",&[]).await?;
        if row.get::<_,Uuid>("id")!=plan.root || row.get::<_,Uuid>("filesystem_uuid")!=plan.filesystem || row.get::<_,i64>("capacity_bytes")!=plan.capacity as i64 || row.get::<_,i64>("reserved_bytes")!=plan.reserved as i64 || row.get::<_,String>("state")!="preparing" {return Err(Error::conflict("Recovery volume budget changed before publication"));}
        let live:i64=tx.query_one("SELECT count(*) FROM volumes WHERE state<>'deleted'",&[]).await?.get(0);
        if live!=plan.items.len()as i64 || proof.status.len()!=plan.items.len() {return Err(Error::conflict("Recovery volume publication set differs"));}
        for item in plan.items {
            let observed=&proof.status.iter().find(|(id,_)|*id==item.volume).ok_or_else(||Error::conflict("Restored volume proof is missing"))?.1;
            let updated=tx.execute("UPDATE volume_allocations SET state='ready',observed_at=now(),updated_at=now() WHERE volume_id=$1 AND root_id=$2 AND project_id=$3 AND owner_id=$4 AND spec=$5 AND capacity_bytes=$6 AND inode_limit=$7 AND metadata_charge=$8 AND state='restored'",&[&item.volume,&plan.root,&item.project,&item.owner,&item.spec,&(item.capacity as i64),&(item.inodes as i64),&(item.metadata as i64)]).await?;
            if updated!=1 {return Err(Error::conflict("Restored allocation changed before publication"));}
            let state=if observed.used_bytes>=item.capacity || observed.used_inodes>=item.inodes {"full"} else {"available"};
            let changed=tx.execute("UPDATE volumes SET backend_id=$2,capacity_bytes=$3,used_bytes=$4,writer_instance_id=NULL,write_fence=write_fence+1,state=$5,revision=revision+1,updated_at=now() WHERE id=$1 AND owner_id=$6 AND spec=$7 AND state IN ('available','full') AND writer_instance_id IS NULL",&[&item.volume,&item.volume.to_string(),&(item.capacity as i64),&(observed.used_bytes as i64),&state,&item.owner,&item.spec]).await?;
            if changed!=1 {return Err(Error::conflict("Logical restored volume changed"));}
        }
        tx.execute("UPDATE volume_storage_root SET state='ready' WHERE id=$1",&[&plan.root]).await?;
    } else if tx.query_one("SELECT EXISTS(SELECT 1 FROM volume_storage_root) OR EXISTS(SELECT 1 FROM volume_allocations) OR EXISTS(SELECT 1 FROM volumes WHERE state<>'deleted')",&[]).await?.get::<_,bool>(0) {
        return Err(Error::conflict("Unexpected volume resources in a rootless restore plan"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backups::tests::Fixture,
        offline::backup::{RawReference, VolumeReference},
        storage::{backup_crypto, backup_transfer},
    };
    #[tokio::test]
    async fn actual_postgres_rebinds_only_reserved_roots_atomically_and_restores_fk_rules() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        // Metadata reservation boundary only; these synthetic UUIDs/limits are
        // not evidence of an XFS mount or physical archive restoration.
        let Some(f) = Fixture::new().await else {
            return;
        };
        let c = f.app.db.connect().await.unwrap();
        let owner: Uuid = c
            .query_one("SELECT created_by FROM backups WHERE id=$1", &[&f.backup])
            .await
            .unwrap()
            .get(0);
        let job = c
            .query_one(
                "SELECT operation_id,credential_id FROM storage_recovery_jobs WHERE backup_id=$1",
                &[&f.backup],
            )
            .await
            .unwrap();
        let operation: Uuid = job.get(0);
        let credential: Uuid = job.get(1);
        let root = Uuid::new_v4();
        let filesystem = Uuid::new_v4();
        c.execute("INSERT INTO volume_storage_root(id,filesystem_uuid,state,capacity_bytes,reserved_bytes) VALUES($1,$2,'ready',134217728,2228224)",&[&root,&filesystem]).await.unwrap();
        let rows = c
            .query("SELECT id,spec FROM volumes ORDER BY id", &[])
            .await
            .unwrap();
        let mut items = Vec::new();
        let mut volumes = Vec::new();
        for (offset, row) in rows.into_iter().enumerate() {
            let volume: Uuid = row.get("id");
            let spec: Value = row.get("spec");
            let source_project = 41 + offset as i32;
            c.execute("INSERT INTO volume_allocations(volume_id,root_id,project_id,owner_id,spec,capacity_bytes,inode_limit,metadata_charge,desired_capacity,desired_inodes,desired_metadata_charge,state,operation_id,credential_id,peer) VALUES($1,$2,$3,$4,$5,1048576,128,65536,1048576,128,65536,'ready',$6,$7,'127.0.0.1')",&[&volume,&root,&source_project,&owner,&spec,&operation,&credential]).await.unwrap();
            items.push(Item {
                volume,
                source_project,
                project: 1001 + offset as i32,
                owner,
                spec: spec.clone(),
                capacity: 1048576,
                inodes: 128,
                metadata: 65536,
            });
            volumes.push(VolumeReference {
                object: backup_transfer::Object {
                    backup: f.backup,
                    object: Uuid::new_v4(),
                    volume,
                    maximum_data: 1048576,
                    maximum_archive: 64 * 1024 * 1024 * 1024,
                },
                spec,
                data_schema_version: None,
                filesystem_uuid: filesystem,
                source_project_id: source_project as u32,
                verified: None,
            });
        }
        c.query_one("SELECT setval('volume_project_ids',1002)", &[])
            .await
            .unwrap();
        let recovery = Uuid::new_v4();
        c.execute("INSERT INTO platform_recovery_state(recovery_id,backup_id,phase,recovery_owner_id) VALUES($1,$2,'rebuilding',$3)",&[&recovery,&f.backup,&owner]).await.unwrap();
        let pid: i32 = f
            .app
            .leader
            .query_one("SELECT pg_backend_pid()", &[])
            .await
            .unwrap()
            .get(0);
        c.query_one("SELECT pg_terminate_backend($1)", &[&pid])
            .await
            .unwrap();
        let session = Arc::new(Session {
            runtime: Runtime::new(f.app.config.clone()),
            db: f.app.db.clone(),
            leader: f.app.db.leader().await.unwrap(),
            root: File::open(f.app.config.root()).unwrap(),
            mutation: Mutex::new(()),
        });
        let hash = digest(b"metadata-only fixture");
        let repository = Uuid::new_v4();
        let receipt = Receipt {
            backup_id: f.backup,
            repository_id: repository,
            manifest_object: Uuid::new_v4(),
            manifest_digest: hash.clone(),
            off_host: false,
        };
        // No S3 access is made by reserve; use the fixture transport solely to
        // construct the coordinator's private, typed metadata context.
        let Some((client, _client_root)) = crate::storage::s3::tests::fixture().await else {
            return;
        };
        let manifest = Manifest {
            format: 1,
            kind: "platform".into(),
            backup_id: f.backup,
            repository_id: repository,
            master_key_reference: digest(&[29; 32]),
            created_at: chrono::Utc::now(),
            stopped_lsn: "0/10".into(),
            metadata: RawReference {
                object: Uuid::new_v4(),
                plaintext: backup_crypto::Summary {
                    bytes: 1,
                    digest: hash.clone(),
                },
                ciphertext: None,
            },
            metadata_summary: metadata_snapshot::Summary {
                snapshot: Uuid::new_v4(),
                schema_version: db::SCHEMA_VERSION,
                postgres_major: 16,
                lsn: "0/11".into(),
                bytes: 1,
                rows: 0,
                sha256: hash[7..].into(),
            },
            installations: vec![],
            images: None,
            volumes,
            blob_count: 0,
            blob_root: None,
            recovery_owners: vec![owner],
            off_host: false,
        };
        let target = Target {
            session,
            recovery,
            receipt,
            manifest,
            repository: Repository {
                id: repository,
                objects: Arc::new(client),
            },
            master: zeroize::Zeroizing::new([29; 32]),
            workspace: None,
            pool: backup_transfer::Pool::default(),
        };
        let plan = Plan {
            high_water: 1000,
            source_root: root,
            source_filesystem: filesystem,
            root: Uuid::new_v4(),
            filesystem: Uuid::new_v4(),
            capacity: 134217728,
            reserved: 2228224,
            items,
        };
        let mut broken = plan.clone();
        broken.items[1].owner = Uuid::new_v4();
        assert!(reserve(&target, &broken).await.is_err());
        assert_eq!(
            c.query_one("SELECT id FROM volume_storage_root", &[])
                .await
                .unwrap()
                .get::<_, Uuid>(0),
            root,
            "late failure must roll back root PK and earlier allocation changes"
        );
        reserve(&target, &plan).await.unwrap();
        reserve(&target, &plan).await.unwrap();
        let rows = c
            .query(
                "SELECT root_id,project_id,state FROM volume_allocations ORDER BY volume_id",
                &[],
            )
            .await
            .unwrap();
        for (row, item) in rows.iter().zip(&plan.items) {
            assert_eq!(row.get::<_, Uuid>("root_id"), plan.root);
            assert_eq!(row.get::<_, i32>("project_id"), item.project);
            assert_eq!(row.get::<_, String>("state"), "restoring");
        }
        let constraint=c.query_one("SELECT condeferrable,condeferred FROM pg_constraint WHERE conrelid='volume_allocations'::regclass AND conname='volume_allocations_root_id_fkey'",&[]).await.unwrap();
        assert!(!constraint.get::<_, bool>(0));
        assert!(!constraint.get::<_, bool>(1));
        assert!(
            f.app.db.check().await.is_err(),
            "metadata reservation never opens recovery gate"
        );
        crate::storage::s3::tests::cleanup(&target.repository.objects, &[]).await;
    }
}
