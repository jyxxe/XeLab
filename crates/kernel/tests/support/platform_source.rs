//! Owned disposable XFS fixture. Host-seeded metadata is not an API/signature
//! acceptance; it supplies a real stopped data volume to the whole-source test.
use serde_json::json;
use std::{
    fs,
    os::unix::fs::{FileExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};
use uuid::Uuid;
use xelab_kernel::{
    db::{self, Db},
    storage::xfs::ProjectStore,
};
pub struct SourceVolume {
    mount: PathBuf,
    image: PathBuf,
    pub volume: Uuid,
    pub filesystem: Uuid,
    mounted: bool,
}
fn command(binary: &str, args: &[&std::ffi::OsStr]) {
    let output = Command::new(binary)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{binary}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
impl SourceVolume {
    pub async fn create(
        root: &Path,
        platform: &Path,
        db: &Db,
        owner: Uuid,
        credential: Uuid,
    ) -> Self {
        assert_eq!(unsafe { libc::geteuid() }, 0);
        assert_eq!(std::env::var("XELAB_OCI_TEST").unwrap(), "1");
        let image = root.join("platform-source.img");
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&image)
            .unwrap();
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .unwrap();
        file.set_len(512 * 1024 * 1024).unwrap();
        file.sync_all().unwrap();
        drop(file);
        let mount = platform.join("volumes");
        fs::create_dir(&mount).unwrap();
        fs::set_permissions(&mount, fs::Permissions::from_mode(0o700)).unwrap();
        command("/usr/sbin/mkfs.xfs", &["-q".as_ref(), image.as_os_str()]);
        command(
            "/usr/bin/mount",
            &[
                "-o".as_ref(),
                "loop,prjquota,nosuid,nodev,noexec".as_ref(),
                image.as_os_str(),
                mount.as_os_str(),
            ],
        );
        fs::set_permissions(&mount, fs::Permissions::from_mode(0o700)).unwrap();
        let mut result = Self {
            mount: mount.clone(),
            image,
            volume: Uuid::new_v4(),
            filesystem: Uuid::nil(),
            mounted: true,
        };
        let mut store = ProjectStore::open(&mount).unwrap();
        let status = store.status().unwrap();
        result.filesystem = status.filesystem_uuid;
        let binding = Uuid::new_v4();
        let limits = store.aligned_limits(8 * 1024 * 1024).unwrap();
        let charge = store.metadata_charge(limits).unwrap();
        let capacity = store
            .available_bytes()
            .unwrap()
            .saturating_sub(64 * 1024 * 1024)
            / status.block_size
            * status.block_size;
        let c = db.connect().await.unwrap();
        let project: i64 = c
            .query_one("SELECT nextval('volume_project_ids')", &[])
            .await
            .unwrap()
            .get(0);
        let project = i32::try_from(project).unwrap();
        let op = db::operation(
            &c,
            owner,
            "offline_component_fixture",
            Some(result.volume.to_string()),
        )
        .await
        .unwrap();
        let spec = json!({"name":"whole-platform data fixture","capacity_bytes":limits.capacity_bytes,"access":"single_writer","scope":"deployment"});
        c.execute("INSERT INTO volume_storage_root(id,filesystem_uuid,state,capacity_bytes,reserved_bytes) VALUES($1,$2,'ready',$3,$4)",&[&binding,&status.filesystem_uuid,&(capacity as i64),&((limits.capacity_bytes+charge)as i64)]).await.unwrap();
        c.execute("INSERT INTO volume_allocations(volume_id,root_id,project_id,owner_id,spec,capacity_bytes,inode_limit,metadata_charge,desired_capacity,desired_inodes,desired_metadata_charge,state,operation_id,credential_id,peer) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$6,$7,$8,'ready',$9,$10,'127.0.0.1')",&[&result.volume,&binding,&project,&owner,&spec,&(limits.capacity_bytes as i64),&(limits.inodes as i64),&(charge as i64),&op,&credential]).await.unwrap();
        c.execute("INSERT INTO volumes(id,owner_id,spec,backend_id,capacity_bytes,used_bytes,write_fence,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,0,1,'available',now(),now())",&[&result.volume,&owner,&spec,&result.volume.to_string(),&(limits.capacity_bytes as i64)]).await.unwrap();
        store
            .namespace_identity(status.filesystem_uuid, binding, true)
            .unwrap();
        store.create(result.volume, project as u32, limits).unwrap();
        store.prepare_data(result.volume, project as u32).unwrap();
        let data = store.data_path(result.volume, project as u32).unwrap();
        fs::write(
            data.join("sentinel"),
            b"whole-platform recovery from encrypted object set",
        )
        .unwrap();
        fs::hard_link(data.join("sentinel"), data.join("same-inode")).unwrap();
        std::os::unix::fs::symlink("sentinel", data.join("relative-link")).unwrap();
        let sparse = fs::File::create(data.join("sparse")).unwrap();
        sparse.set_len(8 * 1024 * 1024 * 1024).unwrap();
        sparse
            .write_all_at(b"tail", 8 * 1024 * 1024 * 1024 - 4)
            .unwrap();
        sparse.sync_all().unwrap();
        drop(sparse);
        store.prepare_data(result.volume, project as u32).unwrap();
        let observed = store.inspect(result.volume, project as u32).unwrap();
        c.execute(
            "UPDATE volumes SET used_bytes=$2 WHERE id=$1",
            &[&result.volume, &(observed.used_bytes as i64)],
        )
        .await
        .unwrap();
        c.execute("UPDATE operations SET state='succeeded',result='{}',revision=revision+1,updated_at=now() WHERE id=$1",&[&op]).await.unwrap();
        drop(store);
        result
    }
    pub fn destroy(mut self) {
        command("/usr/bin/umount", &[self.mount.as_os_str()]);
        self.mounted = false;
        fs::remove_file(&self.image).unwrap();
    }
}
impl Drop for SourceVolume {
    fn drop(&mut self) {
        if self.mounted {
            let _ = Command::new("/usr/bin/umount").arg(&self.mount).status();
        }
    }
}
