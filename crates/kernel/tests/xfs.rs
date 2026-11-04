//! Actual XFS behavior, invoked explicitly by tools/test-xfs.sh on an isolated host.
use std::{
    fs::{self, File},
    io::Write,
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
};
use uuid::Uuid;
use xelab_kernel::storage::xfs::{check, Limits, ProjectStore};

#[test]
#[ignore = "requires root, an isolated host, and two real XFS mounts; use tools/test-xfs.sh"]
fn enforced_projects_survive_reopen_isolate_usage_and_fail_closed() {
    assert_eq!(std::env::var("XELAB_XFS_TEST").unwrap(), "1");
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let root = PathBuf::from(std::env::var_os("XELAB_XFS_ROOT").unwrap());
    let accounting = PathBuf::from(std::env::var_os("XELAB_XFS_ACCOUNTING_ROOT").unwrap());
    assert!(
        check(&accounting).is_err(),
        "accounting without enforcement is unsafe"
    );
    assert!(ProjectStore::open(&accounting).is_err());
    let readonly = PathBuf::from(std::env::var_os("XELAB_XFS_READONLY_ROOT").unwrap());
    assert!(
        check(&readonly).is_err(),
        "read-only bind mount is not usable"
    );
    assert!(ProjectStore::open(&readonly).is_err());
    let status = check(&root).unwrap();
    assert!(!status.filesystem_uuid.is_nil());
    assert_eq!(status.scope, "quota_capability_only");
    assert!(!status.project_namespace_exclusivity_verified && !status.volume_service_ready);
    let shared = root.join("protected-subdirectory");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o700)).unwrap();
    let partial = check(&shared).unwrap();
    assert_eq!(partial.filesystem_uuid, status.filesystem_uuid);
    assert!(!partial.project_namespace_exclusivity_verified && !partial.volume_service_ready);
    fs::remove_dir(shared).unwrap();
    assert!(status.hard_limits_enforced && status.project_accounting);
    let mut store = ProjectStore::open(&root).unwrap();
    assert!(
        ProjectStore::open(&root).is_err(),
        "exclusive backend ownership"
    );
    assert!(
        store.verify_mount_root().is_err(),
        "a second host mount aliases this project namespace"
    );
    assert!(std::process::Command::new("umount")
        .arg(&readonly)
        .status()
        .unwrap()
        .success());
    store.verify_mount_root().unwrap();
    let namespace = Uuid::new_v4();
    store
        .namespace_identity(status.filesystem_uuid, namespace, true)
        .unwrap();
    store
        .namespace_identity(status.filesystem_uuid, namespace, false)
        .unwrap();
    assert!(store
        .namespace_identity(Uuid::new_v4(), namespace, false)
        .is_err());
    assert!(store
        .namespace_identity(status.filesystem_uuid, Uuid::new_v4(), true)
        .is_err());
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let limits = Limits {
        capacity_bytes: 4 * 1024 * 1024,
        inodes: 64,
    };
    assert_eq!(store.create(a, 1001, limits).unwrap().limits, limits);
    store.create(b, 1002, limits).unwrap();
    assert!(store.create(Uuid::new_v4(), 1001, limits).is_err());
    assert!(store.create(a, 1003, limits).is_err());
    assert!(store.inspect(a, 1002).is_err());
    // Nested descendants inherit the project; a large write must hit a real
    // filesystem error, never a Rust-side byte counter or synthetic allocation.
    fs::create_dir(root.join(a.to_string()).join("nested")).unwrap();
    let file_path = root.join(a.to_string()).join("nested/data");
    let mut f = File::create(&file_path).unwrap();
    let data = [0x5a; 65536];
    let mut written = 0u64;
    let failure = loop {
        assert!(written < 8 * 1024 * 1024, "hard quota was bypassed");
        match f.write_all(&data).and_then(|_| f.sync_all()) {
            Ok(()) => written += data.len() as u64,
            Err(e) => break e,
        }
    };
    assert!(
        matches!(failure.raw_os_error(), Some(libc::EDQUOT | libc::ENOSPC)),
        "{failure}"
    );
    assert!(written > 0 && written <= limits.capacity_bytes);
    assert!(store.inspect(a, 1001).unwrap().used_bytes <= limits.capacity_bytes);
    let other = root.join(b.to_string()).join("other");
    File::create(&other)
        .unwrap()
        .write_all(b"independent")
        .unwrap();
    // Cross-project hard links must not import already allocated blocks.
    assert!(fs::hard_link(&file_path, root.join(b.to_string()).join("escape")).is_err());
    assert!(store.expand(a, 1001, limits).is_err());
    assert!(store
        .expand(
            a,
            1001,
            Limits {
                capacity_bytes: limits.capacity_bytes / 2,
                ..limits
            }
        )
        .is_err());
    let larger = Limits {
        capacity_bytes: 8 * 1024 * 1024,
        ..limits
    };
    store.expand(a, 1001, larger).unwrap();
    f.write_all(&data).unwrap();
    f.sync_all().unwrap();
    drop(f);
    // Closing/reopening the backend must discover the same on-disk identity,
    // limits and usage without an in-memory registry.
    drop(store);
    let mut store = ProjectStore::open(&root).unwrap();
    assert_eq!(store.inspect(a, 1001).unwrap().limits, larger);
    assert!(store.inspect(a, 1001).unwrap().used_bytes > 0);
    // Deleting contents returns quota usage while the project remains reserved.
    fs::remove_file(&file_path).unwrap();
    File::open(root.join(a.to_string()).join("nested"))
        .unwrap()
        .sync_all()
        .unwrap();
    // XFS inodegc is asynchronous: fsync of the parent persists the unlink
    // but does not promise quota reclamation has finished. Keep requiring the
    // real quota usage to reach exactly zero, with a bounded observation window.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let used = store.inspect(a, 1001).unwrap().used_bytes;
        if used == 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "quota usage was not reclaimed: {used}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    fs::remove_dir(root.join(a.to_string()).join("nested")).unwrap();
    fs::remove_dir(root.join(a.to_string())).unwrap();
    assert!(
        store.create(Uuid::new_v4(), 1001, limits).is_err(),
        "deleted IDs remain reserved"
    );
    // Zero-length file churn is independently bounded by an inode hard limit.
    let mut count = 0;
    let inode_error = loop {
        assert!(count < 128, "inode quota was bypassed");
        match File::create(root.join(b.to_string()).join(format!("empty-{count}"))) {
            Ok(_) => count += 1,
            Err(e) => break e,
        }
    };
    assert!(count > 0);
    assert!(matches!(
        inode_error.raw_os_error(),
        Some(libc::EDQUOT | libc::ENOSPC)
    ));
    assert!(store.inspect(b, 1002).unwrap().used_inodes <= limits.inodes);
    let link_id = Uuid::new_v4();
    symlink(&accounting, root.join(link_id.to_string())).unwrap();
    assert!(store.create(link_id, 1004, limits).is_err());
    assert!(store.inspect(link_id, 1002).is_err());
    assert!(check(&root.join(link_id.to_string())).is_err());
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(check(&root).is_err());
    assert!(store.inspect(b, 1002).is_err());
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    // Persisted creating intents recover on either side of quota/attribute
    // assignment. The service must supply these identities before publication.
    let recovered = Uuid::new_v4();
    let recovered_root = root.join(recovered.to_string());
    fs::create_dir(&recovered_root).unwrap();
    fs::set_permissions(&recovered_root, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        store
            .recover_create(recovered, 1006, limits)
            .unwrap()
            .limits,
        limits
    );
    store.prepare_data(recovered, 1006).unwrap();
    assert!(store.data_is_empty(recovered, 1006).unwrap());
    assert!(store.data_is_empty(recovered, 1002).is_err());
    let data_path = store.data_path(recovered, 1006).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!(fs::metadata(&recovered_root).unwrap().uid(), 0);
    assert_eq!(fs::metadata(&data_path).unwrap().uid(), 65532);
    fs::write(data_path.join("retained"), b"persistent bytes").unwrap();
    assert!(!store.data_is_empty(recovered, 1006).unwrap());
    assert_eq!(
        store
            .recover_create(recovered, 1006, limits)
            .unwrap()
            .limits,
        limits
    );
    assert_eq!(
        fs::read(data_path.join("retained")).unwrap(),
        b"persistent bytes"
    );
    assert!(store.recover_create(recovered, 1006, larger).is_err());
    let victim = accounting.join("outside-volume");
    fs::write(&victim, b"not deleted").unwrap();
    symlink(&victim, data_path.join("escape")).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !store.remove_data(recovered, 1006).unwrap() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(fs::read(&victim).unwrap(), b"not deleted");
    assert!(store.create(Uuid::new_v4(), 1006, limits).is_err());
    let reserved = Uuid::new_v4();
    let reserved_root = root.join(reserved.to_string());
    fs::create_dir(&reserved_root).unwrap();
    fs::set_permissions(&reserved_root, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(std::process::Command::new("xfs_quota")
        .args([
            "-x",
            "-c",
            "limit -p bhard=4m bsoft=4m ihard=64 isoft=64 1007"
        ])
        .arg(&root)
        .status()
        .unwrap()
        .success());
    assert_eq!(
        store.recover_create(reserved, 1007, limits).unwrap().limits,
        limits
    );
    let unknown = Uuid::new_v4();
    let unknown_root = root.join(unknown.to_string());
    fs::create_dir(&unknown_root).unwrap();
    fs::set_permissions(&unknown_root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(unknown_root.join("unknown"), b"not owned by an intent").unwrap();
    assert!(store.recover_create(unknown, 1008, limits).is_err());
    assert_eq!(
        fs::read(unknown_root.join("unknown")).unwrap(),
        b"not owned by an intent"
    );
    let marker = root.join(".xelab-volume-root");
    let marker_bytes = fs::read(&marker).unwrap();
    fs::remove_file(&marker).unwrap();
    assert!(store
        .namespace_identity(status.filesystem_uuid, namespace, false)
        .is_err());
    assert!(
        store
            .namespace_identity(status.filesystem_uuid, namespace, true)
            .is_err(),
        "a filesystem with existing quota identities cannot be freshly adopted"
    );
    fs::write(&marker, marker_bytes).unwrap();
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
    store
        .namespace_identity(status.filesystem_uuid, namespace, false)
        .unwrap();
    // A backend opened before enforcement is disabled must fail subsequent work.
    assert!(std::process::Command::new("xfs_quota")
        .args(["-x", "-c", "disable -p"])
        .arg(&root)
        .status()
        .unwrap()
        .success());
    assert!(store.inspect(b, 1002).is_err());
    assert!(store.expand(b, 1002, larger).is_err());
    assert!(store.create(Uuid::new_v4(), 1005, limits).is_err());
    println!("XFS: block/inode exhaustion, growth, reopen, isolation, reservation and path rejection passed");
}
