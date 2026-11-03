//! Real compatible S3, AEAD and ordinary directories. Always off_host=false;
//! no public Ready backup, XFS, signed OCI or business-format proof is fabricated.
use super::*;
use crate::storage::{archive, backup_crypto, s3};
use serde_json::json;
use std::{
    fs,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Journal {
    saved: Mutex<Option<SavedUpload>>,
}
#[tonic::async_trait]
impl super::super::backup_transfer::Journal for Journal {
    async fn prepared(
        &self,
        _object: Object,
        upload: &str,
        etags: &[String],
        plaintext: &backup_crypto::Summary,
        ciphertext: &backup_crypto::Summary,
        archive: &archive::Summary,
    ) -> Result<()> {
        *self.saved.lock().unwrap() = Some(SavedUpload {
            id: upload.to_owned(),
            etags: etags.to_vec(),
            plaintext: plaintext.clone(),
            ciphertext: ciphertext.clone(),
            archive: archive.clone(),
        });
        Ok(())
    }
}

#[tokio::test]
async fn actual_s3_format_point_rejects_stale_clones_and_restores_new_directories() {
    let Some((client, root)) = s3::tests::fixture().await else {
        return;
    };
    let client = Arc::new(client);
    let repository_id = Uuid::new_v4();
    client
        .put(
            "xelab-repository.identity",
            serde_json::to_vec(&json!({"format":1,"repository_id":repository_id})).unwrap(),
        )
        .await
        .unwrap();
    let repository = Repository::read_only(repository_id, client.clone())
        .await
        .unwrap();
    let objects = repository.objects.clone();
    let master = [94; 32];
    let backup = Uuid::new_v4();
    let source_volume = Uuid::new_v4();
    let manifest_object = Uuid::new_v4();
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    let payload =
        b"archived business records; actual schema remains the plugin's Verify responsibility";
    fs::write(source.join("store.json"), payload).unwrap();
    let object = Object {
        backup,
        object: Uuid::new_v4(),
        volume: source_volume,
        maximum_data: 1048576,
        maximum_archive: 1048576 + 1048576 / 128 + 65 * 1024 * 1024,
    };
    let journal = Arc::new(Journal::default());
    let pool = Pool::with_journal(journal.clone());
    let upload = objects.initiate(&object.key()).await.unwrap();
    let uploaded = pool
        .upload(
            objects.clone(),
            source,
            object,
            Zeroizing::new(master),
            upload,
            Duration::from_secs(30),
        )
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert!(!uploaded.off_host);
    let saved = journal.saved.lock().unwrap().clone().unwrap();
    let volume_spec = json!({"name":"data","capacity_bytes":1048576,"access":"single_writer","scope":"deployment"});
    let mut plugin: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/secret-manifest.json")).unwrap();
    plugin["requested_rules"] = json!([]);
    plugin["data_schema_version"] = json!("1");
    plugin["capabilities"][0]["side_effect"] = json!("idempotent");
    plugin["volumes"] = json!([{"name":"data","mount_path":"/data","access":"rw","scope":"deployment","minimum_bytes":1048576}]);
    crate::artifact::validate_runtime_profile(&plugin).unwrap();
    let installation = json!({"id":Uuid::new_v4(),"image_digest":digest(b"component image binding"),"manifest_digest":digest(b"component raw manifest binding"),"plugin_id":plugin["plugin_id"],"version":plugin["version"],"manifest":plugin});
    let deployment = Uuid::new_v4();
    let spec = json!({"installation_id":installation["id"],"display_name":"format backup component","enabled":true,"replicas":1,"config":{},"limits":installation["manifest"]["minimum_resources"],"volume_ids":[source_volume]});
    let manifest = json!({"format":1,"backup_id":backup,"deployment_id":deployment,"deployment_spec":spec,"installation":installation,"master_key_reference":digest(&master),"volumes":[{"source_volume_id":source_volume,"object_id":object.object,"volume_spec":volume_spec,"capacity_bytes":1048576,"data_schema_version":"1","transfer":saved}]});
    let manifest_digest = repository
        .write_manifest(backup, manifest_object, &master, &manifest)
        .await
        .unwrap();
    let expected = Expected {
        repository: repository_id,
        backup,
        deployment,
        manifest_object,
        manifest_digest,
        installation,
        manifest,
    };
    let point = Point::read(repository, &master, &expected).await.unwrap();
    let mut other = expected.clone();
    other.repository = Uuid::new_v4();
    assert!(
        Point::read(
            Repository::read_only(repository_id, client.clone())
                .await
                .unwrap(),
            &master,
            &other
        )
        .await
        .is_err(),
        "an admitted repository identity cannot be replaced"
    );
    assert!(!point.off_host());
    assert!(
        point.require_off_host().is_err(),
        "a compatible endpoint cannot satisfy production backup admission"
    );
    assert_eq!(point.volumes(), vec![(source_volume, volume_spec)]);
    let fresh_id = Uuid::new_v4();
    assert_ne!(fresh_id, source_volume);
    let fresh = root.path().join(fresh_id.to_string());
    fs::create_dir(&fresh).unwrap();
    let restored = point
        .restore(
            &pool,
            "data",
            fresh.clone(),
            Zeroizing::new(master),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
    assert!(!restored.off_host);
    assert_eq!(restored, uploaded);
    assert_eq!(fs::read(fresh.join("store.json")).unwrap(), payload);
    assert!(
        !point
            .verify_clone(
                &pool,
                "data",
                fresh.clone(),
                Zeroizing::new(master),
                Duration::from_secs(30)
            )
            .await
            .unwrap()
            .off_host
    );
    assert!(
        point
            .restore(
                &pool,
                "data",
                fresh.clone(),
                Zeroizing::new(master),
                Duration::from_secs(30)
            )
            .await
            .is_err(),
        "a prior private extraction is not an empty retry target"
    );
    assert_eq!(fs::read(fresh.join("store.json")).unwrap(), payload);
    fs::write(
        fresh.join("store.json"),
        b"a newer write absent from this backup",
    )
    .unwrap();
    assert!(
        point
            .verify_clone(
                &pool,
                "data",
                fresh.clone(),
                Zeroizing::new(master),
                Duration::from_secs(30)
            )
            .await
            .is_err(),
        "an available stale backup cannot cover the frozen current clone"
    );
    assert_eq!(
        fs::read(fresh.join("store.json")).unwrap(),
        b"a newer write absent from this backup"
    );
    assert!(point
        .verify_clone(
            &pool,
            "missing",
            fresh.clone(),
            Zeroizing::new(master),
            Duration::from_secs(30)
        )
        .await
        .is_err());
    assert!(point
        .verify_clone(
            &pool,
            "data",
            fresh.clone(),
            Zeroizing::new([95; 32]),
            Duration::from_secs(30)
        )
        .await
        .is_err());
    let mut sealed_keys = vec![object.key(), format!("{backup}/{manifest_object}.sealed")];
    for variant in [
        "deployment",
        "installation",
        "schema",
        "source_identity",
        "duplicate",
        "duplicate_selection",
        "capacity",
        "transfer",
    ] {
        let writer = Repository::read_only(repository_id, client.clone())
            .await
            .unwrap();
        let mut wrong = expected.clone();
        // Normal writes are immutable. Each adversarial authenticated fixture
        // receives its own object identity; never weaken conditional PUT.
        wrong.manifest_object = Uuid::new_v4();
        sealed_keys.push(format!("{backup}/{}.sealed", wrong.manifest_object));
        match variant {
            "deployment" => wrong.manifest["deployment_id"] = json!(Uuid::new_v4()),
            "installation" => wrong.manifest["installation"]["id"] = json!(Uuid::new_v4()),
            "schema" => wrong.manifest["volumes"][0]["data_schema_version"] = json!("2"),
            "source_identity" => {
                wrong.manifest["volumes"][0]["source_volume_id"] = json!(Uuid::nil())
            }
            "duplicate" => {
                let duplicate = wrong.manifest["volumes"][0].clone();
                wrong.manifest["volumes"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            "duplicate_selection" => {
                wrong.manifest["deployment_spec"]["volume_ids"] =
                    json!([source_volume, source_volume])
            }
            "capacity" => wrong.manifest["volumes"][0]["capacity_bytes"] = json!(2097152),
            "transfer" => {
                wrong.manifest["volumes"][0]["transfer"]["plaintext"]["digest"] =
                    json!("untrusted digest")
            }
            _ => unreachable!(),
        }
        wrong.manifest_digest = writer
            .write_manifest(backup, wrong.manifest_object, &master, &wrong.manifest)
            .await
            .unwrap();
        assert!(
            Point::read(writer, &master, &wrong).await.is_err(),
            "even valid AEAD cannot replace the admitted {variant} binding"
        );
    }
    let writer = Repository::read_only(repository_id, client.clone())
        .await
        .unwrap();
    assert_eq!(
        writer
            .write_manifest(backup, manifest_object, &master, &expected.manifest)
            .await
            .unwrap(),
        expected.manifest_digest
    );
    let mut wrong_key = expected.clone();
    wrong_key.manifest_object = Uuid::new_v4();
    sealed_keys.push(format!("{backup}/{}.sealed", wrong_key.manifest_object));
    wrong_key.manifest["master_key_reference"] = json!(digest(&[95; 32]));
    wrong_key.manifest_digest = writer
        .write_manifest(
            backup,
            wrong_key.manifest_object,
            &master,
            &wrong_key.manifest,
        )
        .await
        .unwrap();
    assert!(Point::read(writer, &master, &wrong_key).await.is_err());
    let ciphertext = objects.get(&object.key()).await.unwrap();
    let mut ciphertext = s3::bounded(ciphertext, 8388608).await.unwrap();
    let last = ciphertext.len() - 1;
    ciphertext[last] ^= 1;
    // Deliberate external deletion/replacement in this disposable fixture.
    // Client::put still refuses overwriting an existing object.
    s3::tests::remove(&objects, &[object.key()]).await;
    objects.put(&object.key(), ciphertext).await.unwrap();
    let fresh = root.path().join(Uuid::new_v4().to_string());
    fs::create_dir(&fresh).unwrap();
    assert!(
        point
            .restore(
                &pool,
                "data",
                fresh,
                Zeroizing::new(master),
                Duration::from_secs(30)
            )
            .await
            .is_err(),
        "modified actual encrypted data must fail authentication"
    );
    s3::tests::remove(&objects, &sealed_keys).await;
    s3::tests::cleanup(&client, &["xelab-repository.identity".to_owned()]).await;
    println!("Format-backup component: actual AEAD S3, exact schema/installation/alias binding, stale clone refused, new directory restored and independently read back; off_host=false, no production admission or on-disk business-format claim");
}
