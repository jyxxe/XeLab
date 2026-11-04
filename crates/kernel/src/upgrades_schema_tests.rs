//! Actual PostgreSQL metadata binding only. A fixture Verified label does not
//! prove physical format; the root clone test must supply actual copied bytes.
use super::*;
use crate::{backups::tests::Fixture, contracts::digest};

#[tokio::test]
async fn actual_postgres_clone_binding_requires_verified_plugin_publisher_and_schema() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let app = &f.app;
    let c = app.db.connect().await.unwrap();
    let ctx = app
        .auth
        .authenticate(&c, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    let source: Uuid = c
        .query_one("SELECT id FROM volumes LIMIT 1", &[])
        .await
        .unwrap()
        .get(0);
    let deployment: Uuid = c
        .query_one(
            "SELECT deployment_id FROM backups WHERE id=$1",
            &[&f.backup],
        )
        .await
        .unwrap()
        .get(0);
    let publisher: Uuid = c
        .query_one("SELECT id FROM publishers LIMIT 1", &[])
        .await
        .unwrap()
        .get(0);
    let operation = db::operation(&c, ctx.subject, "schema_binding_fixture", None)
        .await
        .unwrap();
    let root = Uuid::new_v4();
    c.execute("INSERT INTO volume_storage_root(id,filesystem_uuid,state,capacity_bytes) VALUES($1,$2,'ready',67108864)", &[&root,&Uuid::new_v4()]).await.unwrap();
    let spec: Value = c
        .query_one("SELECT spec FROM volumes WHERE id=$1", &[&source])
        .await
        .unwrap()
        .get(0);
    c.execute("INSERT INTO volume_allocations(volume_id,root_id,project_id,owner_id,spec,capacity_bytes,inode_limit,metadata_charge,desired_capacity,desired_inodes,desired_metadata_charge,state,operation_id,credential_id,peer) VALUES($1,$2,nextval('volume_project_ids'),$3,$4,1048576,256,65536,1048576,256,65536,'ready',$5,$6,'127.0.0.1')", &[&source,&root,&ctx.subject,&spec,&operation,&ctx.credential]).await.unwrap();
    let manifest = json!({"plugin_id":"org.xelab.migrating-store","publisher_id":publisher,"data_schema_version":"1"});
    assert!(schema_binding(&c, source, &manifest).await.is_err());
    let mut none = manifest.clone();
    none["data_schema_version"] = json!("none");
    assert!(schema_binding(&c, source, &none).await.unwrap().is_none());
    let manifest_digest = digest(b"metadata binding fixture only");
    c.execute("UPDATE volume_allocations SET data_schema_version='1',schema_state='initializing',schema_deployment_id=$2,schema_plugin_id='org.xelab.migrating-store',schema_publisher_id=$3,schema_manifest_digest=$4,schema_operation_id=$5 WHERE volume_id=$1", &[&source,&deployment,&publisher,&manifest_digest,&operation]).await.unwrap();
    assert!(
        schema_binding(&c, source, &manifest).await.is_err(),
        "an initialization intent cannot authorize cloning known data"
    );
    assert!(schema_binding(&c, source, &none).await.is_err());
    let proof = json!({"metadata_fixture_only":true});
    c.execute(
        "UPDATE volume_allocations SET schema_state='verified',schema_proof=$2 WHERE volume_id=$1",
        &[&source, &proof],
    )
    .await
    .unwrap();
    let binding = schema_binding(&c, source, &manifest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(binding["schema"], "1");
    assert_eq!(binding["operation_id"], operation.to_string());
    assert_eq!(
        binding["proof_digest"],
        digest(&serde_json::to_vec(&proof).unwrap())
    );
    assert!(
        binding.get("schema_proof").is_none(),
        "a chain stores a bounded source proof digest, not recursively nested history"
    );
    for field in ["data_schema_version", "plugin_id", "publisher_id"] {
        let mut wrong = manifest.clone();
        wrong[field] = match field {
            "publisher_id" => json!(Uuid::new_v4()),
            "plugin_id" => json!("org.xelab.other-store"),
            _ => json!("2"),
        };
        assert!(schema_binding(&c, source, &wrong).await.is_err(), "{field}");
    }
    assert!(
        schema_binding(&c, source, &none).await.is_err(),
        "versioned source cannot become an unversioned clone"
    );
}
