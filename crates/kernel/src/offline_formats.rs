//! Historical format identity travels with authenticated metadata and bytes.
//! It never grants publisher trust or revives an old migration credential.
use super::*;
use crate::offline::backup::{FormatBinding, VolumeReference};

impl FormatBinding {
    pub(super) fn validate(&self) -> Result<()> {
        if self.schema.is_empty()
            || self.schema.len() > 64
            || self.schema == "none"
            || self.schema.chars().any(char::is_control)
            || self.deployment.is_nil()
            || self.publisher.is_nil()
            || self.operation.is_nil()
            || self.plugin.is_empty()
            || !valid_digest(&self.manifest_digest)
            || !valid_digest(&self.proof_digest)
        {
            return Err(Error::conflict("Invalid offline volume format binding"));
        }
        Ok(())
    }
}

pub(super) fn reference(volume: &VolumeReference) -> Result<()> {
    match (
        &volume.format_binding,
        volume.data_schema_version.as_deref(),
    ) {
        (None, None | Some("none")) => Ok(()),
        (Some(binding), Some(schema)) if schema == binding.schema => binding.validate(),
        _ => Err(Error::conflict("Offline volume format identity differs")),
    }
}

pub(super) async fn binding(
    c: &(impl GenericClient + Sync),
    volume: Uuid,
) -> Result<Option<FormatBinding>> {
    let row = c.query_one("SELECT data_schema_version,schema_state,schema_deployment_id,schema_plugin_id,schema_publisher_id,schema_manifest_digest,schema_operation_id,schema_proof FROM volume_allocations WHERE volume_id=$1", &[&volume]).await?;
    let schema: Option<String> = row.get("data_schema_version");
    let state: String = row.get("schema_state");
    if matches!(schema.as_deref(), None | Some("none")) {
        if state != "unversioned"
            || row.get::<_, Option<Uuid>>("schema_deployment_id").is_some()
            || row.get::<_, Option<String>>("schema_plugin_id").is_some()
            || row.get::<_, Option<Uuid>>("schema_publisher_id").is_some()
            || row
                .get::<_, Option<String>>("schema_manifest_digest")
                .is_some()
            || row.get::<_, Option<Uuid>>("schema_operation_id").is_some()
            || row.get::<_, Option<Value>>("schema_proof").is_some()
        {
            return Err(Error::conflict(
                "Unversioned offline volume has a format binding",
            ));
        }
        return Ok(None);
    }
    let absent = || Error::conflict("Offline volume lacks a verified format binding");
    let proof = row
        .get::<_, Option<Value>>("schema_proof")
        .filter(Value::is_object)
        .ok_or_else(absent)?;
    let proof = serde_json::to_vec(&proof)?;
    if state != "verified" || proof.len() > 65536 {
        return Err(absent());
    }
    let binding = FormatBinding {
        schema: schema.ok_or_else(absent)?,
        deployment: row
            .get::<_, Option<Uuid>>("schema_deployment_id")
            .ok_or_else(absent)?,
        plugin: row
            .get::<_, Option<String>>("schema_plugin_id")
            .ok_or_else(absent)?,
        publisher: row
            .get::<_, Option<Uuid>>("schema_publisher_id")
            .ok_or_else(absent)?,
        manifest_digest: row
            .get::<_, Option<String>>("schema_manifest_digest")
            .ok_or_else(absent)?,
        operation: row
            .get::<_, Option<Uuid>>("schema_operation_id")
            .ok_or_else(absent)?,
        proof_digest: digest(&proof),
    };
    binding.validate()?;
    // Retained old-format volumes can outlive a deployment's current program.
    // Resolve their original immutable installation, never relabel them using
    // the deployment's latest installation or restore publisher trust.
    let manifests = c.query("SELECT manifest FROM installations WHERE state='installed' AND manifest_digest=$1 AND publisher_id=$2 AND plugin_id=$3 ORDER BY id LIMIT 129", &[&binding.manifest_digest,&binding.publisher,&binding.plugin]).await?;
    if manifests.is_empty() || manifests.len() > 128 {
        return Err(Error::conflict(
            "Offline format installation is unavailable",
        ));
    }
    for row in manifests {
        let manifest: Value = row.get("manifest");
        artifact::validate_runtime_profile(&manifest)?;
        if manifest["data_schema_version"] != binding.schema
            || manifest["plugin_id"] != binding.plugin
            || manifest["publisher_id"] != binding.publisher.to_string()
        {
            return Err(Error::conflict(
                "Offline format differs from its signed scope",
            ));
        }
    }
    Ok(Some(binding))
}

pub(super) async fn check(
    c: &(impl GenericClient + Sync),
    volume: Uuid,
    expected: &Option<FormatBinding>,
) -> Result<()> {
    if &binding(c, volume).await? != expected {
        return Err(Error::conflict(
            "Recovered volume format differs from its authenticated source",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{backups::tests::Fixture, storage::backup_transfer};
    use serde_json::json;

    #[tokio::test]
    async fn actual_postgres_format_binding_rejects_relabeling_and_preserves_revoked_history() {
        // Metadata boundary only. Signed image bytes and actual initialized
        // business contents are exercised separately by the privileged drill.
        let Some(f) = Fixture::new().await else {
            return;
        };
        let c = f.app.db.connect().await.unwrap();
        let row = c
            .query_one(
                "SELECT id,owner_id,spec FROM volumes ORDER BY id LIMIT 1",
                &[],
            )
            .await
            .unwrap();
        let volume: Uuid = row.get("id");
        let owner: Uuid = row.get("owner_id");
        let spec: Value = row.get("spec");
        let job = c.query_one("SELECT operation_id,credential_id,deployment_id FROM storage_recovery_jobs LIMIT 1", &[]).await.unwrap();
        let operation: Uuid = job.get("operation_id");
        let credential: Uuid = job.get("credential_id");
        let deployment: Uuid = job.get("deployment_id");
        let root = Uuid::new_v4();
        let filesystem = Uuid::new_v4();
        c.execute("INSERT INTO volume_storage_root(id,filesystem_uuid,state,capacity_bytes,reserved_bytes) VALUES($1,$2,'ready',134217728,1114112)", &[&root,&filesystem]).await.unwrap();
        c.execute("INSERT INTO volume_allocations(volume_id,root_id,project_id,owner_id,spec,capacity_bytes,inode_limit,metadata_charge,desired_capacity,desired_inodes,desired_metadata_charge,state,operation_id,credential_id,peer) VALUES($1,$2,41,$3,$4,1048576,128,65536,1048576,128,65536,'ready',$5,$6,'127.0.0.1')", &[&volume,&root,&owner,&spec,&operation,&credential]).await.unwrap();
        assert!(binding(&c, volume).await.unwrap().is_none());
        c.execute(
            "UPDATE volume_allocations SET data_schema_version='1' WHERE volume_id=$1",
            &[&volume],
        )
        .await
        .unwrap();
        assert!(
            binding(&c, volume).await.is_err(),
            "a label alone must not become a restorable format"
        );
        let installed = c
            .query_one("SELECT id,publisher_id FROM installations LIMIT 1", &[])
            .await
            .unwrap();
        let installation: Uuid = installed.get("id");
        let publisher: Uuid = installed.get("publisher_id");
        let mut manifest: Value =
            serde_json::from_str(include_str!("../tests/fixtures/secret-manifest.json")).unwrap();
        manifest["publisher_id"] = json!(publisher);
        manifest["data_schema_version"] = json!("1");
        manifest["requested_rules"] = json!([]);
        manifest["volumes"] = json!([{"name":"data","mount_path":"/data","access":"rw","scope":"deployment","minimum_bytes":1048576}]);
        artifact::validate_runtime_profile(&manifest).unwrap();
        assert!(
            artifact::validate_profile(&manifest).is_err(),
            "public installation remains closed"
        );
        let plugin = manifest["plugin_id"].as_str().unwrap();
        let hash = digest(&serde_json::to_vec(&manifest).unwrap());
        c.execute(
            "UPDATE installations SET plugin_id=$2,manifest=$3,manifest_digest=$4 WHERE id=$1",
            &[&installation, &plugin, &manifest, &hash],
        )
        .await
        .unwrap();
        let proof = json!({"metadata_fixture":true,"observed_schema":"1","operation_id":operation,"manifest_digest":hash});
        c.execute("UPDATE volume_allocations SET schema_state='verified',schema_deployment_id=$2,schema_plugin_id=$3,schema_publisher_id=$4,schema_manifest_digest=$5,schema_operation_id=$6,schema_proof=$7 WHERE volume_id=$1", &[&volume,&deployment,&plugin,&publisher,&hash,&operation,&proof]).await.unwrap();
        let expected = binding(&c, volume).await.unwrap();
        let saved = expected.as_ref().unwrap();
        assert_eq!(
            saved.proof_digest,
            digest(&serde_json::to_vec(&proof).unwrap())
        );
        let mut reference_volume = VolumeReference {
            object: backup_transfer::Object {
                backup: f.backup,
                object: Uuid::new_v4(),
                volume,
                maximum_data: 1048576,
                maximum_archive: 64 * 1024 * 1024 * 1024,
            },
            spec,
            data_schema_version: Some("1".into()),
            format_binding: expected.clone(),
            filesystem_uuid: filesystem,
            source_project_id: 41,
            verified: None,
        };
        reference(&reference_volume).unwrap();
        reference_volume.data_schema_version = Some("2".into());
        assert!(reference(&reference_volume).is_err());
        reference_volume.data_schema_version = Some("1".into());
        reference_volume.format_binding = None;
        assert!(reference(&reference_volume).is_err());
        reference_volume.data_schema_version = None;
        reference(&reference_volume).unwrap();
        let encoded = serde_json::to_value(&reference_volume).unwrap();
        assert!(
            encoded.get("format_binding").is_none(),
            "legacy none manifests retain their encoding"
        );
        let legacy: VolumeReference = serde_json::from_value(encoded).unwrap();
        assert!(legacy.format_binding.is_none());
        c.execute(
            "UPDATE publishers SET status='revoked' WHERE id=$1",
            &[&publisher],
        )
        .await
        .unwrap();
        check(&c, volume, &expected).await.unwrap();
        assert_eq!(
            c.query_one("SELECT status FROM publishers WHERE id=$1", &[&publisher])
                .await
                .unwrap()
                .get::<_, String>(0),
            "revoked"
        );
        c.execute("UPDATE volume_allocations SET schema_proof=jsonb_set(schema_proof,'{observed_schema}','\"2\"') WHERE volume_id=$1", &[&volume]).await.unwrap();
        assert!(
            check(&c, volume, &expected).await.is_err(),
            "changing historical proof must block publication"
        );
        c.execute("UPDATE volume_allocations SET schema_proof=$2,data_schema_version='2' WHERE volume_id=$1", &[&volume,&proof]).await.unwrap();
        assert!(
            binding(&c, volume).await.is_err(),
            "an installation cannot authenticate another format"
        );
    }
}
