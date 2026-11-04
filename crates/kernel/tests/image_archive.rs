//! Root-only isolated containerd content-store recovery. Source image references
//! are read only; a second daemon owns a newly created empty root and socket.
use serde_json::json;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{fs::File, process::Command};
use xelab_kernel::{
    artifact,
    config::Config,
    error::{Error, Result},
    runtime::Runtime,
};

#[tokio::test]
#[ignore = "requires root, source OCI images and disposable host containerd; CI"]
async fn real_oci_archive_recovers_into_an_empty_independent_content_store() {
    assert_eq!(
        std::env::var("XELAB_IMAGE_ARCHIVE_TEST").as_deref(),
        Ok("1")
    );
    assert_eq!(unsafe { libc::geteuid() }, 0);
    xelab_kernel::init_crypto();
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let root = tempfile::Builder::new()
        .prefix("xelab-image-backup-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let template: toml::Value =
        toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();
    let mut value = serde_json::to_value(template).unwrap();
    value["storage"]["root_path"] = json!(root.path().join("platform"));
    value["runtime"]["namespace"] = json!("xelab-ci");
    let source = Runtime::new(Config::from_value(value.clone()).unwrap());
    println!(
        "Source containerd versions:\n{}",
        String::from_utf8_lossy(&source.ctr(&["version".into()]).await.unwrap())
    );
    let mut images = vec![
        std::env::var("XELAB_TEST_IMAGE").unwrap(),
        std::env::var("XELAB_TEST_PYTHON_IMAGE").unwrap(),
    ];
    images.sort();
    images.dedup();
    let named = tempfile::NamedTempFile::new_in(root.path()).unwrap();
    let mut archive = File::from_std(named.reopen().unwrap());
    const FIXTURE_LIMIT: u64 = 1024 * 1024 * 1024;
    let exported = source
        .export_images(&images, &mut archive, FIXTURE_LIMIT)
        .await;
    let staged = archive.metadata().await.unwrap().len();
    if exported.is_err() {
        let listing = std::process::Command::new("/usr/bin/tar")
            .arg("-tvf")
            .arg(named.path())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        eprintln!(
            "Independent tar reader status={}\n{}\n{}",
            listing.status,
            String::from_utf8_lossy(&listing.stdout),
            String::from_utf8_lossy(&listing.stderr)
        );
    }
    assert!(
        exported.is_ok(),
        "OCI export failed: {exported:?}; staged_bytes={staged}; admitted_bytes={FIXTURE_LIMIT}"
    );
    let saved = exported.unwrap();
    println!(
        "OCI fixture archive: {} bytes, admitted {} bytes",
        saved.bytes, FIXTURE_LIMIT
    );
    assert!(saved.bytes > 1024);
    assert!(source
        .export_images(&images, &mut archive, 1024 * 1024 * 1024)
        .await
        .is_err());
    let socket = root.path().join("containerd.sock");
    let config = root.path().join("containerd.toml");
    fs::write(&config,format!("version = 2\nroot = \"{}\"\nstate = \"{}\"\ndisabled_plugins = [\"io.containerd.grpc.v1.cri\"]\n[grpc]\naddress = \"{}\"\n",root.path().join("content-root").display(),root.path().join("daemon-state").display(),socket.display())).unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    let daemon_log = tempfile::NamedTempFile::new_in(root.path()).unwrap();
    let mut daemon = Command::new("/usr/bin/containerd")
        .arg("--config")
        .arg(&config)
        .arg("--log-level")
        .arg("error")
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(daemon_log.reopen().unwrap()))
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    value["runtime"]["socket"] = json!(socket);
    value["runtime"]["namespace"] = json!("xelab-cold-image-recovery");
    let target = Runtime::new(Config::from_value(value).unwrap());
    let started_at = Instant::now();
    let mut stage = "isolated daemon readiness";
    let result: Result<()> = async {
        let mut started = false;
        for _ in 0..100 {
            if daemon.try_wait()?.is_some() {
                return Err(Error::unavailable("Isolated containerd exited"));
            }
            if socket.exists() && target.ctr(&["version".into()]).await.is_ok() {
                started = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if !started {
            return Err(Error::unavailable("Isolated containerd did not start"));
        }
        archive_stage(&mut stage, started_at, "create cold namespace");
        target
            .ctr(&[
                "namespaces".into(),
                "create".into(),
                "xelab-cold-image-recovery".into(),
            ])
            .await?;
        archive_stage(&mut stage, started_at, "reject wrong archive digest");
        let mut wrong = saved.clone();
        wrong.sha256 = format!("sha256:{}", "0".repeat(64));
        if target.import_images(&mut archive, &wrong).await.is_ok() {
            return Err(Error::conflict("Wrong archive digest was accepted"));
        }
        let empty = target
            .ctr(&["content".into(), "list".into(), "--quiet".into()])
            .await?;
        if !empty.is_empty() {
            return Err(Error::conflict("Failed archive check imported content"));
        }
        archive_stage(&mut stage, started_at, "cold import");
        target.import_images(&mut archive, &saved).await?;
        archive_stage(&mut stage, started_at, "verify cold image manifests");
        for image in &images {
            target
                .verify_image(image, artifact::image_ref(image)?)
                .await?;
        }
        archive_stage(&mut stage, started_at, "reject second import");
        if target.import_images(&mut archive, &saved).await.is_ok() {
            return Err(Error::conflict(
                "Nonempty namespace accepted a second import",
            ));
        }
        archive_stage(&mut stage, started_at, "resume cold import");
        target.resume_images(&mut archive,&saved).await?;
        archive_stage(&mut stage, started_at, "verify retained cold archive");
        target.verify_restored_images(&mut archive,&saved).await?;
        // Export two admitted names for the same manifest. A distinct empty
        // namespace then shares this daemon's already populated CAS.
        archive_stage(&mut stage, started_at, "tag permitted alias");
        let alias = format!("docker.io/xelab-ci/permitted-alias@{}", artifact::image_ref(&images[0])?);
        target.ctr(&["images".into(), "tag".into(), images[0].clone(), alias.clone()]).await?;
        archive_stage(&mut stage, started_at, "reject unrequested alias resume");
        if target.resume_images(&mut archive,&saved).await.is_ok() {return Err(Error::conflict("Resume accepted an unrequested existing image alias"));}
        let mut alias_names = images.clone();
        alias_names.push(alias);
        alias_names.sort();
        let alias_file = tempfile::NamedTempFile::new()?;
        fs::set_permissions(alias_file.path(), fs::Permissions::from_mode(0o600))?;
        let mut alias_archive = tokio::fs::File::from_std(alias_file.reopen()?);
        archive_stage(&mut stage, started_at, "export permitted aliases");
        let alias_saved = target.export_images(&alias_names, &mut alias_archive, 1024 * 1024 * 1024).await?;
        let mut shared_value=target.config.value.clone();shared_value["runtime"]["namespace"]=json!("xelab-shared-content-recovery");let shared=Runtime::new(Config::from_value(shared_value.clone())?);
        archive_stage(&mut stage, started_at, "import into shared intact CAS");
        target.ctr(&["namespaces".into(),"create".into(),"xelab-shared-content-recovery".into()]).await?;shared.import_images(&mut alias_archive,&alias_saved).await?;
        // Simulate damaged media in this test daemon's private CAS only. The
        // source daemon and every other root remain untouched.
        archive_stage(&mut stage, started_at, "corrupt private CAS layer");
        let manifest:serde_json::Value=serde_json::from_slice(&target.ctr(&["content".into(),"get".into(),artifact::image_ref(&images[0])?.into()]).await?)?;
        let layer=manifest["layers"][0]["digest"].as_str().ok_or_else(||Error::invalid("Fixture image has no layer"))?;
        let path=find_owned_blob(&root.path().join("content-root"),&layer[7..])?;
        corrupt_owned_blob(&path)?;
        archive_stage(&mut stage, started_at, "reject or independently verify repaired CAS");
        shared_value["runtime"]["namespace"]=json!("xelab-damaged-content-recovery");let damaged=Runtime::new(Config::from_value(shared_value)?);target.ctr(&["namespaces".into(),"create".into(),"xelab-damaged-content-recovery".into()]).await?;
        let restored=damaged.import_images(&mut archive,&saved).await;
        // Upstream may repair a blob in a future version. Successful import is
        // allowed only when an independent read proves its bytes were repaired.
        if restored.is_ok()&&file_digest(&path)?!=layer{return Err(Error::conflict("Damaged existing layer was accepted without repair"));}
        println!("Shared CAS: intact existing blobs verified; damaged layer rejected or fully repaired and reverified");
        Ok(())
    }
    .await;
    let _ = daemon.kill().await;
    let _ = daemon.wait().await;
    if result.is_err() {
        eprintln!(
            "OCI archive recovery failed: stage={stage}, elapsed={:.3}s, result={result:?}",
            started_at.elapsed().as_secs_f64()
        );
        archive_failure_diagnostics(root.path(), &daemon_log);
    }
    result.unwrap();
}

fn archive_stage(stage: &mut &'static str, started: Instant, next: &'static str) {
    *stage = next;
    println!(
        "OCI archive recovery stage={next}, elapsed={:.3}s",
        started.elapsed().as_secs_f64()
    );
}

fn archive_failure_diagnostics(root: &std::path::Path, daemon_log: &tempfile::NamedTempFile) {
    use std::{
        io::{Read, Seek, SeekFrom},
        os::fd::AsRawFd,
    };
    let capacity = (|| -> std::io::Result<()> {
        let directory = fs::File::open(root)?;
        let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::fstatvfs(directory.as_raw_fd(), info.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: fstatvfs returned zero and initialized the complete output.
        let info = unsafe { info.assume_init() };
        eprintln!(
            "Private recovery filesystem: blocks={}, free_blocks={}, available_blocks={}, fragment_bytes={}, free_inodes={}",
            info.f_blocks, info.f_bfree, info.f_bavail, info.f_frsize, info.f_favail
        );
        Ok(())
    })();
    if let Err(error) = capacity {
        eprintln!(
            "Recovery capacity diagnostic unavailable: kind={:?}, os_error={:?}",
            error.kind(),
            error.raw_os_error()
        );
    }
    let tail = (|| -> std::io::Result<Vec<u8>> {
        let mut file = daemon_log.reopen()?;
        let bytes = file.metadata()?.len().min(65536);
        file.seek(SeekFrom::End(-(bytes as i64)))?;
        let mut tail = Vec::new();
        file.take(65536).read_to_end(&mut tail)?;
        Ok(tail)
    })();
    match tail {
        Ok(tail) => eprintln!(
            "Isolated recovery containerd stderr (last {} bytes):\n{}",
            tail.len(),
            String::from_utf8_lossy(&tail)
        ),
        Err(error) => eprintln!(
            "Recovery daemon diagnostic unavailable: kind={:?}, os_error={:?}",
            error.kind(),
            error.raw_os_error()
        ),
    }
}

fn find_owned_blob(root: &std::path::Path, name: &str) -> Result<std::path::PathBuf> {
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut entries = 0;
    let mut found = None;
    while let Some((directory, depth)) = pending.pop() {
        if depth > 16 {
            return Err(Error::exhausted());
        }
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            entries += 1;
            if entries > 4096 {
                return Err(Error::exhausted());
            }
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push((entry.path(), depth + 1));
            } else if kind.is_file()
                && entry.file_name() == name
                && found.replace(entry.path()).is_some()
            {
                return Err(Error::conflict("Fixture blob identity is ambiguous"));
            }
        }
    }
    found.ok_or_else(Error::not_found)
}
fn corrupt_owned_blob(path: &std::path::Path) -> Result<()> {
    use std::{
        io::{Read, Seek, SeekFrom, Write},
        os::unix::fs::OpenOptionsExt,
    };
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    file.seek(SeekFrom::Start(17))?;
    let mut byte = [0u8];
    file.read_exact(&mut byte)?;
    byte[0] ^= 1;
    file.seek(SeekFrom::Start(17))?;
    file.write_all(&byte)?;
    file.sync_all()?;
    Ok(())
}
fn file_digest(path: &std::path::Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("sha256:{:x}", hash.finalize()))
}
