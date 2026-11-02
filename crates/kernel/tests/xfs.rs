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
    let status = check(&root).unwrap();
    assert!(status.hard_limits_enforced && status.project_accounting);
    let mut store = ProjectStore::open(&root).unwrap();
    assert!(
        ProjectStore::open(&root).is_err(),
        "exclusive backend ownership"
    );
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
    assert_eq!(store.inspect(a, 1001).unwrap().used_bytes, 0);
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
