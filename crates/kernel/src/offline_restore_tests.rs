//! Disposable-host proof using the earlier actual source capture. The source
//! XFS image has already been destroyed; all payloads must come from S3.
use super::*;
use crate::storage::s3;
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
use serde_json::json;
use std::{
    os::unix::{
        fs::{FileExt, PermissionsExt},
        process::ExitStatusExt,
    },
    process::Stdio,
    time::Duration,
};
const PASSWORD: &str = "fresh standalone platform recovery password";
fn private(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
fn report() -> Value {
    let path = std::env::var("XELAB_PLATFORM_SOURCE_RECEIPT")
        .expect("private report from actual OCI/source phase");
    strict_json(&read_private(Path::new(&path), 1048576).unwrap(), 1048576).unwrap()
}
async fn component_target(config: Config) -> Target {
    require_host_admin().unwrap();
    let info = report();
    let receipt: Receipt = serde_json::from_value(info["receipt"].clone()).unwrap();
    let engine = Path::new(config.text("database", "dsn_file"))
        .parent()
        .unwrap();
    let credentials = engine.join("test-s3.json");
    private(
        &credentials,
        b"{\"access_key_id\":\"xelab-test\",\"secret_access_key\":\"test-only-backup-password\"}",
    );
    let location = s3::Location::aws(info["location"].as_str().unwrap(), "us-east-1")
        .unwrap()
        .compatible(&std::env::var("XELAB_TEST_S3_ENDPOINT").unwrap())
        .unwrap();
    let repo = Repository::read_only(
        receipt.repository_id,
        Arc::new(s3::Client::new(location, s3::Credentials::load(&credentials).unwrap()).unwrap()),
    )
    .await
    .unwrap();
    assert!(!repo.objects.off_host());
    assert!(!receipt.off_host);
    let master = backups::master(&config).unwrap();
    let manifest: Manifest = serde_json::from_value(
        repo.read_manifest(
            receipt.backup_id,
            receipt.manifest_object,
            &master,
            &receipt.manifest_digest,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    manifest
        .validate_receipt(&receipt, &digest(master.as_ref()))
        .unwrap();
    let runtime = Runtime::new(config.clone());
    runtime.retain_daemon_host_lease().unwrap();
    let (dsn, mut key) = config.secrets().unwrap();
    key.zeroize();
    let db = Db::new(dsn).unwrap();
    let c = db.connect().await.unwrap();
    db::verify_migration(&c).await.unwrap();
    let leader = db.leader().await.unwrap();
    let root = protected_directory(&config.root()).unwrap();
    Target {
        session: Arc::new(Session {
            runtime,
            db,
            leader,
            root,
            mutation: Mutex::new(()),
        }),
        recovery: Uuid::new_v4(),
        receipt,
        manifest,
        repository: repo,
        master,
        workspace: None,
        pool: backup_transfer::Pool::default(),
    }
}
fn child_config() -> Config {
    let path = std::env::var("XELAB_RECOVERY_CHILD_CONFIG").unwrap();
    Config::from_value(strict_json(&read_private(Path::new(&path), 65536).unwrap(), 65536).unwrap())
        .unwrap()
}
fn unmount_views(root: &Path) {
    let prefix = root.join("images").to_string_lossy().into_owned() + "/";
    let mut mounts: Vec<String> = fs::read_to_string("/proc/self/mountinfo")
        .unwrap()
        .lines()
        .filter_map(|line| line.split_whitespace().nth(4))
        .filter(|path| path.starts_with(&prefix))
        .map(str::to_owned)
        .collect();
    mounts.sort_by_key(|a| std::cmp::Reverse(a.len()));
    for mount in mounts {
        assert!(
            std::process::Command::new("/usr/bin/umount")
                .arg(&mount)
                .status()
                .unwrap()
                .success(),
            "owned image view failed to unmount: {mount}"
        );
    }
}
#[tokio::test]
#[ignore = "requires owned fresh XFS, PostgreSQL, actual source report, S3 and disposable containerd; tools/test-xfs.sh"]
async fn actual_platform_recovery_rebuilds_fresh_state_and_resumes_after_process_death() {
    assert_eq!(std::env::var("XELAB_XFS_TEST").unwrap(), "1");
    assert_eq!(unsafe { libc::geteuid() }, 0);
    crate::init_crypto();
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    if std::env::var("XELAB_RECOVERY_CHILD_MODE").as_deref() == Ok("crash-after-data") {
        let config = child_config();
        let info = report();
        let owner: Uuid = info["owner_id"].as_str().unwrap().parse().unwrap();
        let mut target = component_target(config.clone()).await;
        target.begin().await.unwrap();
        assert!(target.session.db.check().await.is_err());
        assert!(target
            .import_metadata(Uuid::new_v4(), PASSWORD)
            .await
            .is_err());
        target.import_metadata(owner, PASSWORD).await.unwrap();
        assert!(target.session.db.check().await.is_err());
        assert!(target
            .login_sanity(owner, "incorrect recovery password")
            .await
            .is_err());
        target.login_sanity(owner, PASSWORD).await.unwrap();
        target.restore_images().await.unwrap();
        target.restore_blobs().await.unwrap();
        target.restore_volumes().await.unwrap();
        assert!(
            target.session.db.check().await.is_err(),
            "data reconstruction alone must not open gate"
        );
        let progress = Path::new(config.text("database", "dsn_file"))
            .parent()
            .unwrap()
            .join("progress.json");
        private(
            &progress,
            &serde_json::to_vec(&json!({"recovery_id":target.recovery})).unwrap(),
        );
        // Real OS process death releases every thread/FD and the global lease.
        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) };
        std::process::abort();
    }
    let info = report();
    let owner: Uuid = info["owner_id"].as_str().unwrap().parse().unwrap();
    let volume: Uuid = info["volume_id"].as_str().unwrap().parse().unwrap();
    let source_fs: Uuid = info["source_filesystem"].as_str().unwrap().parse().unwrap();
    let root = std::env::var("XELAB_PLATFORM_RESTORE_ROOT").expect("new script-owned target root");
    let root = Path::new(&root);
    let engine = tempfile::Builder::new()
        .prefix("xelab-platform-engine-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let base = std::env::var("XELAB_TEST_DATABASE_URL").unwrap();
    let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
    let name = format!("platform_target_{}", Uuid::new_v4().simple());
    admin
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let dsn = format!("{base} dbname={name}");
    let db = Db::new(dsn.clone()).unwrap();
    db.migrate().await.unwrap();
    let template: toml::Value =
        toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();
    let mut value = serde_json::to_value(template).unwrap();
    value["storage"]["root_path"] = json!(root);
    value["runtime"]["socket"] = json!(engine.path().join("containerd.sock"));
    value["runtime"]["namespace"] = json!("xelab-platform-target");
    value["server"]["public_origin"] = json!("https://recovered.example.org:8443");
    let key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::new(vec!["fresh-recovery-ca".into()]).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let cert = ca.self_signed(&key).unwrap();
    for (section, field, file, bytes) in [
        ("database", "dsn_file", "database.dsn", dsn.into_bytes()),
        ("security", "token_hmac_key_file", "hmac.key", vec![43; 32]),
        ("security", "master_key_file", "master.key", vec![42; 32]),
        (
            "security",
            "workload_ca_key_file",
            "ca.key",
            key.serialize_pem().into_bytes(),
        ),
        (
            "security",
            "workload_ca_cert_file",
            "ca.crt",
            cert.pem().into_bytes(),
        ),
        (
            "server",
            "tls_key_file",
            "server.key",
            key.serialize_pem().into_bytes(),
        ),
        (
            "server",
            "tls_cert_file",
            "server.crt",
            cert.pem().into_bytes(),
        ),
    ] {
        let path = engine.path().join(file);
        private(&path, &bytes);
        value[section][field] = json!(path);
    }
    let config = Config::from_value(value).unwrap();
    let config_file = engine.path().join("config.json");
    private(&config_file, &serde_json::to_vec(&config.value).unwrap());
    let daemon_config = engine.path().join("containerd.toml");
    private(&daemon_config,format!("version = 2\nroot = \"{}\"\nstate = \"{}\"\ndisabled_plugins = [\"io.containerd.grpc.v1.cri\"]\n[grpc]\naddress = \"{}\"\n",engine.path().join("content").display(),engine.path().join("state").display(),config.text("runtime","socket")).as_bytes());
    let mut daemon = tokio::process::Command::new("/usr/bin/containerd")
        .arg("--config")
        .arg(&daemon_config)
        .arg("--log-level")
        .arg("error")
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let runtime = Runtime::new(config.clone());
    let result:Result<()>=async {
        let mut started=false;for _ in 0..100 {if daemon.try_wait()?.is_some(){return Err(Error::unavailable("Owned recovery daemon exited"));}
if runtime.ctr(&["version".into()]).await.is_ok(){started=true;break;}tokio::time::sleep(Duration::from_millis(50)).await;}if !started{return Err(Error::unavailable("Owned recovery daemon did not start"));}
        runtime.ctr(&["namespaces".into(),"create".into(),"xelab-platform-target".into()]).await?;
        assert!(Target::open(config.clone(),serde_json::from_value(info["receipt"].clone())?,false).await.is_err());
        let status=tokio::process::Command::new(std::env::current_exe()?).args(["--ignored","--exact","offline::restore::tests::actual_platform_recovery_rebuilds_fresh_state_and_resumes_after_process_death","--nocapture"]).env("XELAB_RECOVERY_CHILD_MODE","crash-after-data").env("XELAB_RECOVERY_CHILD_CONFIG",&config_file).status().await?;
        assert_eq!(status.signal(),Some(libc::SIGABRT),"child must reach the intentional post-data crash");assert!(db.check().await.is_err());
        let progress=strict_json(&read_private(&engine.path().join("progress.json"),4096)?,4096)?;let recovery:Uuid=progress["recovery_id"].as_str().unwrap().parse().map_err(|_|Error::invalid("Invalid test recovery identity"))?;
        let mut target=component_target(config.clone()).await;target.resume(recovery).await?;target.import_metadata(owner,PASSWORD).await?;target.restore_images().await?;target.restore_blobs().await?;target.restore_volumes().await?;
        let c=db.connect().await?;let row=c.query_one("SELECT root_id,project_id FROM volume_allocations WHERE volume_id=$1",&[&volume]).await?;let project:i32=row.get("project_id");
        let mut store=crate::storage::xfs::ProjectStore::open(&root.join("volumes"))?;let filesystem=store.status()?.filesystem_uuid;assert_ne!(filesystem,source_fs);store.namespace_identity(filesystem,row.get("root_id"),false)?;let data=store.data_path(volume,project as u32)?;drop(store);
        let sentinel=data.join("sentinel");let before=fs::metadata(&sentinel)?;fs::write(&sentinel,b"tampered")?;assert!(target.complete(owner,PASSWORD).await.is_err());assert!(db.check().await.is_err(),"corrupt target must retain gate");
        fs::write(&sentinel,b"whole-platform recovery from encrypted object set")?;let times=[libc::timespec{tv_sec:before.atime(),tv_nsec:before.atime_nsec()},libc::timespec{tv_sec:before.mtime(),tv_nsec:before.mtime_nsec()}];let path=std::ffi::CString::new(sentinel.as_os_str().as_bytes()).unwrap();assert_eq!(unsafe{libc::utimensat(libc::AT_FDCWD,path.as_ptr(),times.as_ptr(),libc::AT_SYMLINK_NOFOLLOW)},0);
        let completed=target.complete(owner,PASSWORD).await?;assert_eq!(completed.gate,"ready");assert!(!completed.runtime_started);assert!(!completed.off_host);db.check().await?;
        assert_eq!(fs::metadata(data.join("same-inode"))?.ino(),fs::metadata(&sentinel)?.ino());assert_eq!(fs::read_link(data.join("relative-link"))?,Path::new("sentinel"));let sparse=fs::File::open(data.join("sparse"))?;assert_eq!(sparse.metadata()?.len(),8*1024*1024*1024);let mut tail=[0;4];sparse.read_exact_at(&mut tail,8*1024*1024*1024-4)?;assert_eq!(&tail,b"tail");
        assert_eq!(c.query_one("SELECT count(*) FROM credentials WHERE status='active'",&[]).await?.get::<_,i64>(0),0);assert_eq!(c.query_one("SELECT count(*) FROM deployments WHERE spec->>'enabled'='true'",&[]).await?.get::<_,i64>(0),0);assert!(c.query_one("SELECT write_fence FROM volumes WHERE id=$1",&[&volume]).await?.get::<_,i64>(0)>=3);
        assert_eq!(target.completed_receipt().await?.recovery_id,recovery);
        println!("Whole recovery: actual encrypted metadata/OCI/Blob/XFS, fresh DB/CAS/filesystem and keys, real process death/resume, target corruption refused, old authority revoked; compatible off_host=false, not independent-host/AWS acceptance");
        Ok(())
    }.await;
    unmount_views(root);
    let _ = daemon.kill().await;
    let _ = daemon.wait().await;
    result.unwrap();
}
