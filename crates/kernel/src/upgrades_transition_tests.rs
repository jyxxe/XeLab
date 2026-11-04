//! Real PostgreSQL parent/child journals and policy. These metadata fixtures
//! are NOT physical data formats, signed images, stopped runtimes or available
//! off-host backups. The fixture backup remains creating/off_host=false.
use super::*;
use crate::{backups::tests::Fixture, contracts::digest};

#[tokio::test]
async fn actual_postgres_format_child_binds_parent_and_private_historical_scopes() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let app = &f.app;
    let mut c = app.db.connect().await.unwrap();
    let ctx = app
        .auth
        .authenticate(&c, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    let publisher: Uuid = c
        .query_one("SELECT id FROM publishers LIMIT 1", &[])
        .await
        .unwrap()
        .get(0);
    let historical: Uuid = c
        .query_one("SELECT id FROM volumes LIMIT 1", &[])
        .await
        .unwrap()
        .get(0);
    let mut manifest: Value =
        serde_json::from_str(include_str!("../tests/fixtures/secret-manifest.json")).unwrap();
    manifest["requested_rules"] = json!([]);
    manifest["publisher_id"] = json!(publisher);
    manifest["volumes"] = json!([{"name":"data","mount_path":"/data","scope":"deployment","access":"rw","minimum_bytes":1048576}]);
    let source = Uuid::new_v4();
    let target = Uuid::new_v4();
    let mut metadata = Vec::new();
    for (installation, version, schema) in [(source, "1.0.0", "1"), (target, "2.0.0", "2")] {
        manifest["version"] = json!(version);
        manifest["data_schema_version"] = json!(schema);
        let hash = digest(&serde_json::to_vec(&manifest).unwrap());
        c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,'org.xelab.echo-python',$2,$3,$4,$4,$5,'installed',now(),now())", &[&installation,&version,&publisher,&hash,&manifest]).await.unwrap();
        metadata.push(super::super::installation(&c, installation).await.unwrap());
    }
    let deployment = Uuid::new_v4();
    let candidate = Uuid::new_v4();
    let old = Uuid::new_v4();
    let new = Uuid::new_v4();
    let scratch = Uuid::new_v4();
    let original = json!({"installation_id":source,"display_name":"format journal fixture","enabled":true,"replicas":1,"limits":manifest["minimum_resources"],"config":{},"volume_ids":[old]});
    let mut next = original.clone();
    next["installation_id"] = json!(target);
    next["volume_ids"] = json!([new]);
    c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,$4,'ready',now(),now())", &[&deployment,&ctx.subject,&source,&original]).await.unwrap();
    c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,created_at,updated_at) VALUES($1,$2,2,0,'pending','unknown',$3,now(),now())", &[&candidate,&deployment,&manifest["minimum_resources"]]).await.unwrap();
    let operation = db::operation(
        &c,
        ctx.subject,
        "upgradeDeployment",
        Some(deployment.to_string()),
    )
    .await
    .unwrap();
    let paths = Policy::new(&c, ctx.peer)
        .credential(ctx.credential, 0)
        .await
        .unwrap();
    let authority = serde_json::to_value(&paths).unwrap();
    c.execute("INSERT INTO deployment_upgrade_jobs(operation_id,deployment_id,action,credential_id,subject_id,peer,authority,source_installation_id,target_installation_id,source_metadata,target_metadata,original_spec,target_spec,instances,candidate_id,candidate_generation,strategy,state,backup_id,source_revision) VALUES($1,$2,'plugin.upgrade',$3,$4,'127.0.0.1',$5,$6,$7,$8,$9,$10,$11,'[]',$12,2,'recreate','stopped',$13,1)", &[&operation,&deployment,&ctx.credential,&ctx.subject,&authority,&source,&target,&metadata[0],&metadata[1],&original,&next,&candidate,&f.backup]).await.unwrap();
    let root = Uuid::new_v4();
    let filesystem = Uuid::new_v4();
    c.execute("INSERT INTO volume_storage_root(id,filesystem_uuid,state,capacity_bytes) VALUES($1,$2,'ready',134217728)", &[&root,&filesystem]).await.unwrap();
    let business = json!({"name":"data","capacity_bytes":1048576,"scope":"deployment","access":"single_writer"});
    c.execute("INSERT INTO volumes(id,owner_id,spec,backend_id,capacity_bytes,state,created_at,updated_at) VALUES($1,$2,$3,$4,1048576,'available',now(),now())", &[&old,&ctx.subject,&business,&old.to_string()]).await.unwrap();
    for (volume, spec, state) in [
        (old, business.clone(), "ready"),
        (new, business.clone(), "restoring"),
        (
            scratch,
            json!({"name":"kernel-schema-scratch","capacity_bytes":67108864,"scope":"deployment","access":"single_writer"}),
            "restoring",
        ),
    ] {
        let capacity = spec["capacity_bytes"].as_i64().unwrap();
        c.execute("INSERT INTO volume_allocations(volume_id,root_id,project_id,owner_id,spec,capacity_bytes,inode_limit,metadata_charge,desired_capacity,desired_inodes,desired_metadata_charge,state,operation_id,credential_id,peer) VALUES($1,$2,nextval('volume_project_ids'),$3,$4,$5,256,65536,$5,256,65536,$6,$7,$8,'127.0.0.1')", &[&volume,&root,&ctx.subject,&spec,&capacity,&state,&operation,&ctx.credential]).await.unwrap();
    }
    c.execute("INSERT INTO deployment_upgrade_volumes(operation_id,ordinal,source_volume_id,target_volume_id,source_spec,state) VALUES($1,0,$2,$3,$4,'reserved')", &[&operation,&old,&new,&business]).await.unwrap();
    let project: i32 = c
        .query_one(
            "SELECT project_id FROM volume_allocations WHERE volume_id=$1",
            &[&new],
        )
        .await
        .unwrap()
        .get(0);
    // Synthetic registration exercises journal/auth only. capture/read cannot
    // accept it: no remote objects and the actual backup is still unavailable.
    let registration = json!({"credential":ctx.credential,"subject":ctx.subject,"backup_operation":Uuid::new_v4(),"backup_owner":ctx.subject,"object_reference":"metadata fixture only","master_key_id":digest(b"metadata fixture"),"volume_ids":[historical],"expected":{"repository":Uuid::new_v4(),"backup":f.backup,"deployment":deployment,"manifest_object":Uuid::new_v4(),"manifest_digest":digest(b"metadata fixture"),"installation":metadata[0],"manifest":{"metadata_fixture_only":true}}});
    let physical =
        json!([{"id":new,"root":root,"filesystem":filesystem,"project":project,"spec":business}]);
    c.execute("INSERT INTO volume_schema_jobs(operation_id,kind,deployment_id,instance_id,generation,installation_id,credential_id,subject_id,peer,authority,deployment_spec,installation_metadata,source_schema,target_schema,scratch_volume_id,volume_ids,volume_plan,state,backup_id,backup_registration,expires_at) SELECT $1,'upgrade',$2,$3,2,$4,$5,$6,'127.0.0.1',$7,$8,$9,'1','2',$10,$11,$12,'planning',$13,$14,expires_at FROM deployment_upgrade_jobs WHERE operation_id=$1", &[&operation,&deployment,&candidate,&target,&ctx.credential,&ctx.subject,&authority,&next,&metadata[1],&scratch,&json!([new]),&physical,&f.backup,&registration]).await.unwrap();
    let parent = parent(&c, operation).await.unwrap();
    let child = c.query_one("SELECT j.*,host(j.peer) AS approved_peer FROM volume_schema_jobs j WHERE operation_id=$1", &[&operation]).await.unwrap();
    validate_parent_binding(&parent, &child).unwrap();
    for changed in [
        json!({"credential_id":Uuid::new_v4()}),
        json!({"peer":"127.0.0.2"}),
        json!({"generation":3}),
        json!({"source_schema":"9"}),
        json!({"instance_id":Uuid::new_v4()}),
        json!({"volume_ids":[old]}),
        json!({"backup_id":Uuid::new_v4()}),
        json!({"volume_fence":7}),
        json!({"authority":[]}),
        json!({"installation_metadata":{}}),
        json!({"deployment_spec":original}),
    ] {
        // PostgreSQL typed snapshots allow one field to differ independently;
        // these are never persisted or passed to filesystem/runtime mutation.
        let wrong = c.query_one("SELECT snapshot.*,host(snapshot.peer) AS approved_peer FROM volume_schema_jobs j CROSS JOIN LATERAL jsonb_populate_record(NULL::volume_schema_jobs,to_jsonb(j)||$2::jsonb) snapshot WHERE j.operation_id=$1", &[&operation,&changed]).await.unwrap();
        assert_eq!(
            validate_parent_binding(&parent, &wrong)
                .unwrap_err()
                .message,
            "Format child differs from its original upgrade intent"
        );
    }
    schema_jobs::authorize_replay(&c, &ctx, operation)
        .await
        .unwrap();
    for permitted in [
        vec![old, new],
        vec![old, new, scratch],
        vec![new, scratch, historical],
        vec![old, new, scratch, historical],
    ] {
        let mut narrowed = paths.clone();
        for path in &mut narrowed {
            if path.rule["action"] == "volume.read" {
                path.rule["resource"] = json!({"kind":"volume","match":"ids","ids":permitted});
            }
        }
        let tx = c.transaction().await.unwrap();
        tx.execute(
            "UPDATE deployment_upgrade_jobs SET authority=$2 WHERE operation_id=$1",
            &[&operation, &serde_json::to_value(&narrowed).unwrap()],
        )
        .await
        .unwrap();
        tx.execute(
            "UPDATE volume_schema_jobs SET authority=$2 WHERE operation_id=$1",
            &[&operation, &serde_json::to_value(&narrowed).unwrap()],
        )
        .await
        .unwrap();
        assert_eq!(
            super::super::authorize_replay(&tx, &ctx, operation)
                .await
                .is_ok()
                && schema_jobs::authorize_replay(&tx, &ctx, operation)
                    .await
                    .is_ok(),
            permitted.len() == 4,
            "source/private/scratch/historical rights cannot replace one another"
        );
        tx.rollback().await.unwrap();
    }
    let revoked = c.transaction().await.unwrap();
    db::auth_lock(&revoked, true).await.unwrap();
    revoked
        .execute(
            "UPDATE credentials SET status='revoked' WHERE id=$1",
            &[&ctx.credential],
        )
        .await
        .unwrap();
    assert!(schema_jobs::authorize_replay(&revoked, &ctx, operation)
        .await
        .is_err());
    revoked.rollback().await.unwrap();
    assert!(!c
        .query_one("SELECT off_host FROM backups WHERE id=$1", &[&f.backup])
        .await
        .unwrap()
        .get::<_, bool>(0));
    assert_eq!(
        c.query_one("SELECT count(*) FROM volumes WHERE id=$1", &[&new])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    assert_eq!(
        c.query_one(
            "SELECT schema_state FROM volume_allocations WHERE volume_id=$1",
            &[&new]
        )
        .await
        .unwrap()
        .get::<_, String>(0),
        "unversioned"
    );
}
