//! Root-only isolated containerd content-store recovery. Source image references
//! are read only; a second daemon owns a newly created empty root and socket.
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt, process::Stdio, time::Duration};
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
    let mut daemon = Command::new("/usr/bin/containerd")
        .arg("--config")
        .arg(&config)
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
    value["runtime"]["socket"] = json!(socket);
    value["runtime"]["namespace"] = json!("xelab-cold-image-recovery");
    let target = Runtime::new(Config::from_value(value).unwrap());
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
        target
            .ctr(&[
                "namespaces".into(),
                "create".into(),
                "xelab-cold-image-recovery".into(),
            ])
            .await?;
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
        target.import_images(&mut archive, &saved).await?;
        for image in &images {
            target
                .verify_image(image, artifact::image_ref(image)?)
                .await?;
        }
        if target.import_images(&mut archive, &saved).await.is_ok() {
            return Err(Error::conflict(
                "Nonempty namespace accepted a second import",
            ));
        }
        Ok(())
    }
    .await;
    let _ = daemon.kill().await;
    let _ = daemon.wait().await;
    result.unwrap();
}
