//! Full offline source capture: native metadata, exact immutable Blob objects,
//! actual OCI content, and each stopped logical volume. Only the final encrypted
//! manifest publishes the complete set; intermediate objects are not a backup.
use super::workspace::{Header, ObjectJournal, ObjectPlan, Preparation, Workspace};
use super::*;
use crate::{
    backup_repository::{Repository, Settings},
    backups,
    storage::{
        backup_crypto,
        backup_transfer::{self, raw},
        blob::Store,
    },
};
use std::{collections::BTreeMap, sync::Arc};
use zeroize::Zeroizing;
const META_RESERVATION: u64 = 1024 * 1024 * 1024;
const HEADROOM: u64 = 128 * 1024 * 1024;
const MAX_BLOBS: usize = 65536;
const MAX_OBJECT: u64 = 64 * 1024 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawReference {
    pub object: Uuid,
    pub plaintext: backup_crypto::Summary,
    pub ciphertext: Option<backup_crypto::Summary>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageReference {
    pub data: RawReference,
    pub archive: crate::runtime::ImageArchive,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeReference {
    pub object: backup_transfer::Object,
    pub spec: Value,
    pub data_schema_version: Option<String>,
    pub filesystem_uuid: Uuid,
    pub source_project_id: u32,
    pub verified: Option<backup_transfer::Verified>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub kind: String,
    pub backup_id: Uuid,
    pub repository_id: Uuid,
    pub master_key_reference: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub stopped_lsn: String,
    pub metadata: RawReference,
    pub metadata_summary: metadata_snapshot::Summary,
    pub installations: Vec<InstallationImage>,
    pub images: Option<ImageReference>,
    pub volumes: Vec<VolumeReference>,
    pub blob_count: u64,
    pub blob_root: Option<Uuid>,
    pub recovery_owners: Vec<Uuid>,
    pub off_host: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub backup_id: Uuid,
    pub repository_id: Uuid,
    pub manifest_object: Uuid,
    pub manifest_digest: String,
    pub off_host: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    manifest: Manifest,
    manifest_object: Uuid,
    manifest_digest: Option<String>,
}
struct Blob {
    id: Uuid,
    plain: backup_crypto::Summary,
}

fn lsn(value: &str) -> Result<u64> {
    let (high, low) = value
        .split_once('/')
        .ok_or_else(|| Error::invalid("Invalid snapshot LSN"))?;
    if high.is_empty()
        || low.is_empty()
        || high.len() > 8
        || low.len() > 8
        || !high
            .bytes()
            .chain(low.bytes())
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::invalid("Invalid snapshot LSN"));
    }
    let high = u32::from_str_radix(high, 16).map_err(|_| Error::invalid("Invalid snapshot LSN"))?;
    let low = u32::from_str_radix(low, 16).map_err(|_| Error::invalid("Invalid snapshot LSN"))?;
    Ok((u64::from(high) << 32) | u64::from(low))
}
fn check_summary(value: &backup_crypto::Summary, maximum: u64) -> Result<()> {
    if value.bytes > maximum || !valid_digest(&value.digest) {
        return Err(Error::invalid("Invalid platform object summary"));
    }
    Ok(())
}
impl State {
    fn validate(&self, header: &Header, complete: bool) -> Result<()> {
        let m = &self.manifest;
        if m.format != 1
            || m.kind != "platform"
            || m.backup_id != header.snapshot
            || m.repository_id != header.repository
            || m.master_key_reference != header.master_key_reference
            || m.backup_id.is_nil()
            || m.repository_id.is_nil()
            || !valid_digest(&m.master_key_reference)
            || m.metadata_summary.snapshot.is_nil()
            || m.metadata_summary.schema_version != db::SCHEMA_VERSION
            || !(10..=99).contains(&m.metadata_summary.postgres_major)
            || m.metadata_summary.bytes == 0
            || m.metadata_summary.bytes > META_RESERVATION
            || m.metadata_summary.rows > 10_000_000
            || m.metadata.plaintext.bytes != m.metadata_summary.bytes
            || m.metadata.plaintext.digest != format!("sha256:{}", m.metadata_summary.sha256)
            || lsn(&m.metadata_summary.lsn)? < lsn(&m.stopped_lsn)?
            || m.blob_count > MAX_BLOBS as u64
            || m.blob_root.is_some_and(|id| id.is_nil())
            || (m.blob_count > 0 && m.blob_root.is_none())
            || m.recovery_owners.is_empty()
            || m.recovery_owners.len() > 128
            || m.installations.len() > 128
            || m.volumes.len() > MAX_VOLUMES as usize
        {
            return Err(Error::conflict(
                "Platform manifest differs from its source checkpoint",
            ));
        }
        let mut owners = BTreeSet::new();
        if m.recovery_owners
            .iter()
            .any(|id| id.is_nil() || !owners.insert(*id))
        {
            return Err(Error::invalid("Platform recovery Owner set is invalid"));
        }
        let mut objects = BTreeSet::new();
        if self.manifest_object.is_nil() || !objects.insert(self.manifest_object) {
            return Err(Error::invalid("Invalid platform manifest object"));
        }
        let mut raw = vec![&m.metadata];
        let mut installations = BTreeSet::new();
        let mut references = BTreeSet::new();
        for image in &m.installations {
            if image.installation.is_nil()
                || !installations.insert(image.installation)
                || artifact::image_ref(&image.image)? != image.image_digest
                || !valid_digest(&image.manifest_digest)
            {
                return Err(Error::invalid("Invalid platform installation reference"));
            }
            references.insert(image.image.clone());
        }
        match &m.images {
            Some(image) => {
                if references.is_empty()
                    || image.archive.images != references.into_iter().collect::<Vec<_>>()
                    || image.archive.bytes == 0
                    || image.archive.bytes != image.data.plaintext.bytes
                    || image.archive.sha256 != image.data.plaintext.digest
                {
                    return Err(Error::conflict(
                        "Platform OCI archive differs from its installations",
                    ));
                }
                raw.push(&image.data);
            }
            None if !references.is_empty() => {
                return Err(Error::conflict("Platform OCI content is missing"))
            }
            None => {}
        }
        for item in raw {
            if item.object.is_nil() || !objects.insert(item.object) {
                return Err(Error::invalid("Platform object identities repeat"));
            }
            check_summary(&item.plaintext, MAX_OBJECT)?;
            if let Some(cipher) = &item.ciphertext {
                check_summary(cipher, MAX_OBJECT + MAX_OBJECT / 65536 * 20 + 1024)?;
            } else if complete {
                return Err(Error::conflict("Platform object was not remotely verified"));
            }
        }
        let mut volumes = BTreeSet::new();
        let mut projects = BTreeSet::new();
        let mut filesystem = None;
        for volume in &m.volumes {
            crate::contracts::validate("VolumeSpec", &volume.spec)?;
            let object = &volume.object;
            if object.backup != m.backup_id
                || object.object.is_nil()
                || !objects.insert(object.object)
                || object.volume.is_nil()
                || !volumes.insert(object.volume)
                || volume.filesystem_uuid.is_nil()
                || volume.source_project_id == 0
                || volume.source_project_id > i32::MAX as u32
                || !projects.insert(volume.source_project_id)
                || filesystem
                    .replace(volume.filesystem_uuid)
                    .is_some_and(|old| old != volume.filesystem_uuid)
                || volume
                    .data_schema_version
                    .as_deref()
                    .is_some_and(|v| v != "none")
                || object.maximum_archive != MAX_OBJECT
                || object.maximum_data == 0
                || object.maximum_data > 63 * 1024 * 1024 * 1024
                || volume.spec["capacity_bytes"].as_u64() != Some(object.maximum_data)
            {
                return Err(Error::conflict("Platform volume plan is inconsistent"));
            }
            if let Some(verified) = &volume.verified {
                check_summary(&verified.plaintext, object.maximum_archive)?;
                check_summary(
                    &verified.ciphertext,
                    MAX_OBJECT + MAX_OBJECT / 65536 * 20 + 1024,
                )?;
                if verified.archive.data_bytes > object.maximum_data
                    || verified.off_host != m.off_host
                {
                    return Err(Error::conflict("Platform volume verification differs"));
                }
            } else if complete {
                return Err(Error::conflict("Platform volume was not remotely verified"));
            }
        }
        if self
            .manifest_digest
            .as_ref()
            .is_some_and(|v| !valid_digest(v))
        {
            return Err(Error::invalid("Invalid published platform manifest digest"));
        }
        Ok(())
    }
}

async fn check_secret_keys(c: &(impl GenericClient + Sync), key: &str) -> Result<()> {
    if c.query_one(
        "SELECT EXISTS(SELECT 1 FROM secrets WHERE encryption_key_id<>$1)",
        &[&key],
    )
    .await?
    .get::<_, bool>(0)
    {
        return Err(Error::conflict(
            "Platform Secrets require an unavailable historical master key",
        ));
    }
    Ok(())
}

pub async fn create(config: Config, allow_downtime: bool) -> Result<Receipt> {
    require_host_admin()?;
    if !allow_downtime {
        return Err(Error::invalid(
            "Offline maintenance requires explicit downtime",
        ));
    }
    // Fail missing backend/key configuration before taking the source offline.
    let settings = Settings::from_host(&config)?;
    let _ = settings.client()?;
    let master = backups::master(&config)?;
    let session = Arc::new(Session::open(config, allow_downtime).await?);
    let c = session.db.connect().await?;
    source_preflight(&c).await?;
    let repository = settings.register(&c).await?;
    let repo = settings.bind(&session.db, repository).await?;
    if !repo.objects.off_host() {
        return Err(Error::unavailable(
            "Platform backups require an off-host repository",
        ));
    }
    capture(session, repo, master).await
}
fn fresh(ids: &mut BTreeSet<Uuid>) -> Uuid {
    loop {
        let id = Uuid::new_v4();
        if ids.insert(id) {
            return id;
        }
    }
}
async fn blobs(session: &Session) -> Result<(Vec<Blob>, Option<Uuid>, Option<Store>)> {
    let c = session.db.connect().await?;
    let rows = c
        .query(
            "SELECT id,size_bytes,digest,backend_reference FROM blobs ORDER BY id LIMIT 65537",
            &[],
        )
        .await?;
    if rows.len() > MAX_BLOBS {
        return Err(Error::exhausted());
    }
    let binding = c
        .query_opt("SELECT id,state FROM blob_storage_root", &[])
        .await?;
    if binding
        .as_ref()
        .is_some_and(|r| r.get::<_, String>("state") != "ready")
        || (!rows.is_empty() && binding.is_none())
    {
        return Err(Error::conflict(
            "Platform Blob storage is not completely registered",
        ));
    }
    let root = binding.as_ref().map(|r| r.get::<_, Uuid>("id"));
    let store = if let Some(id) = root {
        let store = Store::open(&session.runtime.config.root())?;
        store.identity(id, false)?;
        Some(store)
    } else {
        None
    };
    let mut result = Vec::new();
    for row in rows {
        let id: Uuid = row.get("id");
        let bytes: i64 = row.get("size_bytes");
        let hash: String = row.get("digest");
        if id.is_nil()
            || !(0..=67108864).contains(&bytes)
            || !valid_digest(&hash)
            || row.get::<_, String>("backend_reference") != id.to_string()
        {
            return Err(Error::conflict("Platform Blob metadata is inconsistent"));
        }
        result.push(Blob {
            id,
            plain: backup_crypto::Summary {
                bytes: bytes as u64,
                digest: hash,
            },
        });
    }
    Ok((result, root, store))
}
async fn image_reservation(runtime: &Runtime, images: &[InstallationImage]) -> Result<u64> {
    let mut contents = BTreeMap::<String, u64>::new();
    for image in images {
        let bytes = runtime
            .ctr(&["content".into(), "get".into(), image.image_digest.clone()])
            .await?;
        if digest(&bytes) != image.image_digest {
            return Err(Error::conflict("Image content changed before reservation"));
        }
        if contents
            .insert(image.image_digest.clone(), bytes.len() as u64)
            .is_some_and(|old| old != bytes.len() as u64)
        {
            return Err(Error::conflict("Image descriptor sizes disagree"));
        }
        let value = strict_json(&bytes, 2097152)?;
        let layers = value["layers"]
            .as_array()
            .ok_or_else(|| Error::invalid("Invalid installed image layers"))?;
        for item in std::iter::once(&value["config"]).chain(layers) {
            let id = item["digest"]
                .as_str()
                .ok_or_else(|| Error::invalid("Image descriptor has no digest"))?;
            let size = item["size"]
                .as_u64()
                .ok_or_else(|| Error::invalid("Image descriptor has no length"))?;
            if !valid_digest(id) || size > 64 * 1024 * 1024 * 1024 {
                return Err(Error::invalid("Image descriptor exceeds backup bounds"));
            }
            if contents
                .insert(id.to_owned(), size)
                .is_some_and(|old| old != size)
            {
                return Err(Error::conflict("Image descriptor sizes disagree"));
            }
        }
    }
    let mut bytes = 3 * 1024 * 1024u64;
    for size in contents.values() {
        bytes = bytes
            .checked_add(512 + size.div_ceil(512) * 512)
            .ok_or_else(Error::exhausted)?;
    }
    if bytes > 64 * 1024 * 1024 * 1024 {
        return Err(Error::exhausted());
    }
    Ok(bytes)
}
/// Capture under an already exclusive host session. The receipt always reports
/// the actual client's off-host evidence; the production CLI requires AWS.
/// A compatible local service can exercise the same capture protocol without
/// being represented as off-host acceptance.
pub async fn capture(
    session: Arc<Session>,
    repo: Repository,
    master: Zeroizing<[u8; 32]>,
) -> Result<Receipt> {
    require_host_admin()?;
    let c = session.db.connect().await?;
    source_preflight(&c).await?;
    check_secret_keys(&c, &digest(master.as_ref())).await?;
    let (blobs, blob_root, store) = blobs(&session).await?;
    let images = session.installation_images(&c).await?;
    let image_bytes = if images.is_empty() {
        0
    } else {
        image_reservation(&session.runtime, &images).await?
    };
    let backup = Uuid::new_v4();
    let key_ref = digest(master.as_ref());
    let workspace = Arc::new(Workspace::create(
        &session.root,
        Header {
            format: 1,
            snapshot: backup,
            repository: repo.id,
            master_key_reference: key_ref.clone(),
            phase: "preparing".into(),
            context: Value::Null,
        },
    )?);
    let result=async{
        // Reserve physical staging bytes before draining any source process.
        let metadata_file=workspace.reserve_file(false,META_RESERVATION,image_bytes+HEADROOM)?;
        let image_file=if image_bytes>0{Some(workspace.reserve_file(true,image_bytes,HEADROOM)?)}else{None};
        let frozen=session.freeze_source().await?;
        workspace.update_header("frozen",Value::Null)?;
        let sources=frozen.volume_sources().await?;
        let mut output=tokio::fs::File::from_std(metadata_file);
        let metadata_summary=frozen.export_metadata(&mut output).await?;output.sync_all().await?;drop(output);
        let mut used:BTreeSet<Uuid>=blobs.iter().map(|b|b.id).collect();
        let metadata=RawReference{object:fresh(&mut used),plaintext:backup_crypto::Summary{bytes:metadata_summary.bytes,digest:format!("sha256:{}",metadata_summary.sha256)},ciphertext:None};
        let images=if let Some(file)=image_file{let mut refs:Vec<String>=frozen.images().iter().map(|i|i.image.clone()).collect();refs.sort();refs.dedup();let mut file=tokio::fs::File::from_std(file);let archive=session.runtime.export_images(&refs,&mut file,image_bytes).await?;Some(ImageReference{data:RawReference{object:fresh(&mut used),plaintext:backup_crypto::Summary{bytes:archive.bytes,digest:archive.sha256.clone()},ciphertext:None},archive})}else{None};
        let mut volumes=Vec::new();
        for source in &sources{volumes.push(VolumeReference{object:backup_transfer::Object{backup,object:fresh(&mut used),volume:source.id,maximum_data:source.capacity_bytes,maximum_archive:64*1024*1024*1024},spec:source.spec.clone(),data_schema_version:source.data_schema_version.clone(),filesystem_uuid:source.filesystem_uuid,source_project_id:source.project_id,verified:None});}
        let owners=c.query("SELECT id FROM accounts WHERE kind='user' AND parent_id IS NULL AND independent_owner AND status<>'archived' ORDER BY id LIMIT 129",&[]).await?;
        if owners.is_empty()||owners.len()>128{return Err(Error::conflict("Recovery Owner candidate set exceeds its bound"));}
        let state=State{manifest:Manifest{format:1,kind:"platform".into(),backup_id:backup,repository_id:repo.id,master_key_reference:key_ref.clone(),created_at:chrono::Utc::now(),stopped_lsn:frozen.stopped_lsn().to_owned(),metadata,metadata_summary,installations:frozen.images().to_vec(),images,volumes,blob_count:blobs.len()as u64,blob_root,recovery_owners:owners.into_iter().map(|r|r.get(0)).collect(),off_host:repo.objects.off_host()},manifest_object:fresh(&mut used),manifest_digest:None};
        if serde_json::to_vec(&state)?.len()>900000{return Err(Error::exhausted());}
        state.validate(&workspace.header()?,false)?;
        register(&workspace,&state,&blobs)?;
        workspace.update_header("transferring",serde_json::to_value(&state)?)?;
        let journal=Arc::new(ObjectJournal{workspace:workspace.clone(),session:Arc::downgrade(&session)});
        let pool=backup_transfer::Pool::with_journal(journal.clone());let raw=pool.raw(Some(journal));
        upload_file(&workspace,&raw,&repo,backup,&state.manifest.metadata,workspace.staged_file(false)?,&master).await?;
        if let Some(image)=&state.manifest.images{upload_file(&workspace,&raw,&repo,backup,&image.data,workspace.staged_file(true)?,&master).await?;}
        for (source,volume) in sources.iter().zip(&state.manifest.volumes){let upload=initiate(&workspace,&repo,backup,volume.object.object).await?;let value=pool.upload(repo.objects.clone(),source.path.clone(),volume.object,Zeroizing::new(*master),upload,std::time::Duration::from_secs(3600))?.wait().await?;workspace.verified(volume.object.object,&value.plaintext,&value.ciphertext)?;session.check_leader()?;}
        for blob in &blobs{let store=store.as_ref().ok_or_else(||Error::conflict("Blob root was lost"))?;let reference=RawReference{object:blob.id,plaintext:blob.plain.clone(),ciphertext:None};upload_file(&workspace,&raw,&repo,backup,&reference,store.open_object(blob.id)?,&master).await?;session.check_leader()?;}
        finish(&session,&workspace,&repo,&master,state,false).await
    }.await;
    if result.is_err() {
        if let Ok(header) = workspace.header() {
            let all = workspace
                .records()
                .is_ok_and(|r| !r.is_empty() && r.iter().all(|r| r.preparation.is_some()));
            if header.phase != "committing" && !all {
                let _ = workspace.update_header("failed", header.context);
            }
        }
    }
    result
}
fn register(workspace: &Workspace, state: &State, blobs: &[Blob]) -> Result<()> {
    workspace.register(
        state.manifest.metadata.object,
        ObjectPlan::Raw {
            expected: state.manifest.metadata.plaintext.clone(),
        },
    )?;
    if let Some(image) = &state.manifest.images {
        workspace.register(
            image.data.object,
            ObjectPlan::Raw {
                expected: image.data.plaintext.clone(),
            },
        )?;
    }
    for volume in &state.manifest.volumes {
        workspace.register(
            volume.object.object,
            ObjectPlan::Volume {
                volume: volume.object.volume,
                maximum_data: volume.object.maximum_data,
                maximum_archive: volume.object.maximum_archive,
            },
        )?;
    }
    for blob in blobs {
        workspace.register(
            blob.id,
            ObjectPlan::Raw {
                expected: blob.plain.clone(),
            },
        )?;
    }
    Ok(())
}
async fn initiate(
    workspace: &Workspace,
    repo: &Repository,
    backup: Uuid,
    object: Uuid,
) -> Result<String> {
    let key = format!("{backup}/{object}.sealed");
    workspace.upload_starting(object)?;
    let upload = match repo.objects.initiate(&key).await {
        Ok(upload) => upload,
        Err(error) => {
            // Retain the starting intent even if the response was lost; one
            // best-effort inventory is not proof that no late MPU can appear.
            if let Ok((ids, _)) = repo.objects.pending_uploads(&key).await {
                for id in ids {
                    let _ = repo.objects.abort(&key, &id).await;
                }
            }
            return Err(error);
        }
    };
    if let Err(error) = workspace.upload_started(object, &upload) {
        let _ = repo.objects.abort(&key, &upload).await;
        return Err(error);
    }
    Ok(upload)
}
async fn upload_file(
    workspace: &Workspace,
    pool: &raw::RawPool,
    repo: &Repository,
    backup: Uuid,
    reference: &RawReference,
    file: File,
    master: &[u8; 32],
) -> Result<()> {
    let source = raw::Source::new(file, reference.plaintext.clone())?;
    let upload = initiate(workspace, repo, backup, reference.object).await?;
    let value = pool
        .upload_file(
            repo.objects.clone(),
            source,
            raw::Object {
                backup,
                object: reference.object,
                maximum_bytes: reference.plaintext.bytes,
            },
            Zeroizing::new(*master),
            upload,
            std::time::Duration::from_secs(3600),
        )?
        .wait()
        .await?;
    workspace.verified(reference.object, &value.plaintext, &value.ciphertext)
}
async fn finish(
    session: &Session,
    workspace: &Workspace,
    repo: &Repository,
    master: &[u8; 32],
    mut state: State,
    recover: bool,
) -> Result<Receipt> {
    state.validate(&workspace.header()?, false)?;
    if state.manifest.off_host != repo.objects.off_host() {
        return Err(Error::conflict("Platform repository evidence changed"));
    }
    let records = workspace.records()?;
    let expected = state.manifest.blob_count as usize
        + 1
        + usize::from(state.manifest.images.is_some())
        + state.manifest.volumes.len();
    if records.len() != expected
        || records
            .iter()
            .any(|r| r.preparation.is_none() || (!recover && r.phase != "verified"))
    {
        return Err(Error::conflict(
            "Not every platform object has a complete preparation",
        ));
    }
    let pool = backup_transfer::Pool::default();
    let raw = pool.raw(None);
    for record in records {
        if record.object == state.manifest_object || record.object.is_nil() {
            return Err(Error::conflict(
                "Platform manifest object overlaps its data",
            ));
        }
        session.check_leader()?;
        match record
            .preparation
            .ok_or_else(|| Error::conflict("Missing object preparation"))?
        {
            Preparation::Raw(saved) => {
                let ObjectPlan::Raw { expected } = &record.plan else {
                    return Err(Error::conflict("Raw preparation type differs"));
                };
                if expected != &saved.plaintext {
                    return Err(Error::conflict("Raw preparation source digest differs"));
                }
                if record.object != state.manifest.metadata.object
                    && !state
                        .manifest
                        .images
                        .as_ref()
                        .is_some_and(|i| i.data.object == record.object)
                    && (expected.bytes > 67108864
                        || state
                            .manifest
                            .volumes
                            .iter()
                            .any(|v| v.object.object == record.object))
                {
                    return Err(Error::conflict("Unexpected raw platform object"));
                }
                if recover {
                    raw.recover(
                        repo.objects.clone(),
                        raw::Object {
                            backup: state.manifest.backup_id,
                            object: record.object,
                            maximum_bytes: expected.bytes,
                        },
                        Zeroizing::new(*master),
                        saved.clone(),
                        std::time::Duration::from_secs(3600),
                    )?
                    .wait()
                    .await?;
                }
                if record.object == state.manifest.metadata.object {
                    if saved.plaintext != state.manifest.metadata.plaintext {
                        return Err(Error::conflict("Metadata preparation differs"));
                    }
                    state.manifest.metadata.ciphertext = Some(saved.ciphertext.clone());
                } else if let Some(image) = state
                    .manifest
                    .images
                    .as_mut()
                    .filter(|i| i.data.object == record.object)
                {
                    if saved.plaintext != image.data.plaintext {
                        return Err(Error::conflict("Image preparation differs"));
                    }
                    image.data.ciphertext = Some(saved.ciphertext.clone());
                }
                workspace.verified(record.object, &saved.plaintext, &saved.ciphertext)?;
            }
            Preparation::Volume(saved) => {
                let volume = state
                    .manifest
                    .volumes
                    .iter_mut()
                    .find(|v| v.object.object == record.object)
                    .ok_or_else(|| Error::conflict("Unexpected volume object"))?;
                let expected = ObjectPlan::Volume {
                    volume: volume.object.volume,
                    maximum_data: volume.object.maximum_data,
                    maximum_archive: volume.object.maximum_archive,
                };
                if record.plan != expected {
                    return Err(Error::conflict("Volume preparation plan differs"));
                }
                let verified = if recover {
                    pool.recover(
                        repo.objects.clone(),
                        volume.object,
                        Zeroizing::new(*master),
                        saved.clone(),
                        std::time::Duration::from_secs(3600),
                    )?
                    .wait()
                    .await?
                } else {
                    backup_transfer::Verified {
                        plaintext: saved.plaintext.clone(),
                        ciphertext: saved.ciphertext.clone(),
                        archive: saved.archive.clone(),
                        off_host: repo.objects.off_host(),
                    }
                };
                workspace.verified(record.object, &saved.plaintext, &saved.ciphertext)?;
                volume.verified = Some(verified);
            }
        }
    }
    if state.manifest.metadata.ciphertext.is_none()
        || state
            .manifest
            .images
            .as_ref()
            .is_some_and(|i| i.data.ciphertext.is_none())
        || state.manifest.volumes.iter().any(|v| v.verified.is_none())
    {
        return Err(Error::conflict(
            "Platform manifest object set is incomplete",
        ));
    }
    state.validate(&workspace.header()?, true)?;
    workspace.update_header("committing", serde_json::to_value(&state)?)?;
    session.check_leader()?;
    let value = serde_json::to_value(&state.manifest)?;
    let hash = repo
        .write_manifest(
            state.manifest.backup_id,
            state.manifest_object,
            master,
            &value,
        )
        .await?;
    session.check_leader()?;
    state.manifest_digest = Some(hash.clone());
    workspace.update_header("succeeded", serde_json::to_value(&state)?)?;
    workspace.remove_staged_data()?;
    Ok(Receipt {
        backup_id: state.manifest.backup_id,
        repository_id: state.manifest.repository_id,
        manifest_object: state.manifest_object,
        manifest_digest: hash,
        off_host: repo.objects.off_host(),
    })
}
pub async fn resume(config: Config, backup: Uuid, allow_downtime: bool) -> Result<Receipt> {
    require_host_admin()?;
    if !allow_downtime {
        return Err(Error::invalid(
            "Offline maintenance requires explicit downtime",
        ));
    }
    let settings = Settings::from_host(&config)?;
    let master = backups::master(&config)?;
    let session = Session::open(config, allow_downtime).await?;
    let c = session.db.connect().await?;
    let repository = settings.register(&c).await?;
    let repo = settings.bind(&session.db, repository).await?;
    if !repo.objects.off_host() {
        return Err(Error::unavailable(
            "Platform backups require an off-host repository",
        ));
    }
    let workspace = Workspace::resume(&session.root, backup, repo.id, &digest(master.as_ref()))?;
    let header = workspace.header()?;
    if !matches!(
        header.phase.as_str(),
        "transferring" | "committing" | "succeeded"
    ) {
        if matches!(header.phase.as_str(), "preparing" | "frozen") {
            workspace.update_header("failed", header.context)?;
        }
        return Err(Error::conflict("Platform snapshot is not resumable"));
    }
    let state: State = serde_json::from_value(header.context.clone())?;
    state.validate(&header, header.phase == "succeeded")?;
    if state.manifest.backup_id != backup
        || state.manifest.repository_id != repo.id
        || state.manifest.master_key_reference != digest(master.as_ref())
    {
        return Err(Error::conflict("Platform snapshot is not resumable"));
    }
    if header.phase == "succeeded" {
        let hash = state
            .manifest_digest
            .ok_or_else(|| Error::conflict("Published manifest receipt is missing"))?;
        let actual = repo
            .read_manifest(backup, state.manifest_object, &master, &hash)
            .await?;
        if actual != serde_json::to_value(&state.manifest)? {
            return Err(Error::conflict("Published platform manifest differs"));
        }
        session.check_leader()?;
        workspace.remove_staged_data()?;
        return Ok(Receipt {
            backup_id: backup,
            repository_id: repo.id,
            manifest_object: state.manifest_object,
            manifest_digest: hash,
            off_host: repo.objects.off_host(),
        });
    }
    if header.phase == "transferring" {
        let expected = state.manifest.blob_count as usize
            + 1
            + usize::from(state.manifest.images.is_some())
            + state.manifest.volumes.len();
        let records = workspace.records()?;
        if records.len() != expected || records.iter().any(|r| r.preparation.is_none()) {
            // The old process no longer owns this directory/host lease. Never
            // read the mutable source again to fill a missing old checkpoint.
            workspace.update_header("failed", header.context)?;
            return Err(Error::conflict(
                "Interrupted source set is incomplete; snapshot failed and accepts upload cleanup",
            ));
        }
    }
    finish(&session, &workspace, &repo, &master, state, true).await
}

#[derive(Serialize)]
pub struct CleanupReceipt {
    backup_id: Uuid,
    objects_checked: usize,
    uploads_aborted: usize,
    more_pending: bool,
    repeat_required: bool,
}
async fn cleanup_inner(
    session: &Session,
    workspace: &Workspace,
    repo: &Repository,
) -> Result<CleanupReceipt> {
    let header = workspace.header()?;
    if header.phase != "failed" {
        return Err(Error::conflict(
            "Only failed platform snapshots accept upload cleanup",
        ));
    }
    let mut result = CleanupReceipt {
        backup_id: header.snapshot,
        objects_checked: 0,
        uploads_aborted: 0,
        more_pending: false,
        repeat_required: true,
    };
    for record in workspace.records()? {
        if record.phase == "initializing" {
            continue;
        }
        session.check_leader()?;
        let key = format!("{}/{}.sealed", header.snapshot, record.object);
        if let Some(id) = &record.upload {
            repo.objects.abort(&key, id).await?;
            result.uploads_aborted += 1;
        }
        let (ids, more) = repo.objects.pending_uploads(&key).await?;
        for id in ids {
            repo.objects.abort(&key, &id).await?;
            result.uploads_aborted += 1;
        }
        result.objects_checked += 1;
        result.more_pending |= more;
    }
    session.check_leader()?;
    // Keep every intent. Late initiation responses require later sweeps;
    // published objects and local forensic/staging data are never removed here.
    Ok(result)
}
pub async fn cleanup(config: Config, backup: Uuid, allow_downtime: bool) -> Result<CleanupReceipt> {
    require_host_admin()?;
    if !allow_downtime {
        return Err(Error::invalid(
            "Offline maintenance requires explicit downtime",
        ));
    }
    let settings = Settings::from_host(&config)?;
    let master = backups::master(&config)?;
    let session = Session::open(config, allow_downtime).await?;
    let c = session.db.connect().await?;
    let repository = settings.register(&c).await?;
    let repo = settings.bind(&session.db, repository).await?;
    let workspace = Workspace::resume(&session.root, backup, repo.id, &digest(master.as_ref()))?;
    cleanup_inner(&session, &workspace, &repo).await
}

#[cfg(test)]
#[path = "offline_backup_tests.rs"]
mod tests;
