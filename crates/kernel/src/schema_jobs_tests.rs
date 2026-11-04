//! Real PostgreSQL safety checks. No fixture row is physical format, XFS,
//! stopped-runtime, available backup or production migration evidence.
use super::*;
use crate::backups::tests::Fixture;

#[tokio::test]
async fn actual_postgres_schema_checkpoints_and_original_authority() {
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
    let installation = c
        .query_one(
            "SELECT id,publisher_id,plugin_id FROM installations LIMIT 1",
            &[],
        )
        .await
        .unwrap();
    let installed: Uuid = installation.get("id");
    let deployment = Uuid::new_v4();
    let instance = Uuid::new_v4();
    let source = Uuid::new_v4();
    let scratch = Uuid::new_v4();
    let root = Uuid::new_v4();
    let filesystem = Uuid::new_v4();
    let mut manifest: Value =
        serde_json::from_str(include_str!("../tests/fixtures/secret-manifest.json")).unwrap();
    crate::artifact::validate_profile(&manifest).unwrap();
    manifest["data_schema_version"] = json!("1");
    // Admission remains deliberately closed until the full format worker is
    // ready. These metadata checks never bypass that public profile gate.
    assert!(crate::artifact::validate_profile(&manifest).is_err());
    let limits = manifest["minimum_resources"].clone();
    let disabled = json!({"installation_id":installed,"display_name":"schema metadata fixture","enabled":false,"replicas":1,"limits":limits,"config":{},"volume_ids":[source]});
    c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,$4,'disabled',now(),now())", &[&deployment,&ctx.subject,&installed,&disabled]).await.unwrap();
    c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,created_at,updated_at) VALUES($1,$2,2,0,'pending','unknown',$3,now(),now())", &[&instance,&deployment,&limits]).await.unwrap();
    let before: i64 = c
        .query_one("SELECT reserved_cpu_millis FROM node_budget", &[])
        .await
        .unwrap()
        .get(0);
    // Capacity metadata only: no controller or runtime is started by this test.
    c.execute("UPDATE node_budget SET admission_enabled=true", &[])
        .await
        .unwrap();
    let denied = c.transaction().await.unwrap();
    assert_eq!(
        admission::reserve(&denied, instance)
            .await
            .unwrap_err()
            .message,
        "Instance does not match an enabled deployment",
        "the enabled intent must precede the reservation in the same transaction"
    );
    denied.rollback().await.unwrap();
    assert_eq!(
        c.query_one("SELECT reserved_cpu_millis FROM node_budget", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        before
    );
    let operation = db::operation(
        &c,
        ctx.subject,
        "setDeploymentEnabled",
        Some(deployment.to_string()),
    )
    .await
    .unwrap();
    let mut enabled = disabled;
    enabled["enabled"] = json!(true);
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, false).await.unwrap();
    tx.query_one("SELECT id FROM node_budget FOR UPDATE", &[])
        .await
        .unwrap();
    tx.execute(
        "UPDATE deployments SET spec=$2 WHERE id=$1",
        &[&deployment, &enabled],
    )
    .await
    .unwrap();
    admission::reserve(&tx, instance).await.unwrap();
    admission::reserve(&tx, instance).await.unwrap();
    assert_eq!(
        tx.query_one("SELECT reserved_cpu_millis FROM node_budget", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        before + limits["cpu_millis"].as_i64().unwrap()
    );
    tx.execute("INSERT INTO volume_storage_root(id,filesystem_uuid,state,capacity_bytes,reserved_bytes) VALUES($1,$2,'ready',134217728,68157440)", &[&root,&filesystem]).await.unwrap();
    let volume_spec = json!({"name":"business","capacity_bytes":1048576,"access":"single_writer","scope":"deployment"});
    tx.execute("INSERT INTO volumes(id,owner_id,spec,backend_id,capacity_bytes,state,created_at,updated_at) VALUES($1,$2,$3,$4,1048576,'available',now(),now())", &[&source,&ctx.subject,&volume_spec,&source.to_string()]).await.unwrap();
    for (volume, spec, state, capacity) in [
        (source, volume_spec.clone(), "ready", 1048576i64),
        (
            scratch,
            json!({"name":"kernel-schema-scratch","capacity_bytes":SCRATCH_BYTES,"access":"single_writer","scope":"deployment"}),
            "restoring",
            SCRATCH_BYTES as i64,
        ),
    ] {
        tx.execute("INSERT INTO volume_allocations(volume_id,root_id,project_id,owner_id,spec,capacity_bytes,inode_limit,metadata_charge,desired_capacity,desired_inodes,desired_metadata_charge,state,operation_id,credential_id,peer) VALUES($1,$2,nextval('volume_project_ids'),$3,$4,$5,256,65536,$5,256,65536,$6,$7,$8,'127.0.0.1')", &[&volume,&root,&ctx.subject,&spec,&capacity,&state,&operation,&ctx.credential]).await.unwrap();
    }
    let project: i32 = tx
        .query_one(
            "SELECT project_id FROM volume_allocations WHERE volume_id=$1",
            &[&source],
        )
        .await
        .unwrap()
        .get(0);
    let metadata = json!({"id":installed,"plugin_id":installation.get::<_,String>("plugin_id"),"publisher_id":installation.get::<_,Uuid>("publisher_id"),"manifest_digest":crate::contracts::digest(b"metadata fixture only"),"manifest":manifest});
    let authority = serde_json::to_value(
        Policy::new(&tx, ctx.peer)
            .credential(ctx.credential, 0)
            .await
            .unwrap(),
    )
    .unwrap();
    let volume_plan = json!([{"id":source,"spec":volume_spec,"root":root,"filesystem":filesystem,"project":project,"schema_state":"unversioned","schema":null}]);
    tx.execute("INSERT INTO volume_schema_jobs(operation_id,kind,deployment_id,instance_id,generation,installation_id,credential_id,subject_id,peer,authority,deployment_spec,installation_metadata,source_schema,target_schema,scratch_volume_id,volume_ids,volume_plan,state) VALUES($1,'bootstrap',$2,$3,2,$4,$5,$6,'127.0.0.1',$7,$8,$9,'none','1',$10,$11,$12,'planning')", &[&operation,&deployment,&instance,&installed,&ctx.credential,&ctx.subject,&authority,&enabled,&metadata,&scratch,&json!([source]),&volume_plan]).await.unwrap();
    tx.execute(
        "INSERT INTO deployment_storage_holds(deployment_id,operation_id) VALUES($1,$2)",
        &[&deployment, &operation],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO volume_holds(volume_id,operation_id,kind) VALUES($1,$2,'migration')",
        &[&source, &operation],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    authorize_replay(&c, &ctx, operation).await.unwrap();
    // An original backup foreign key alone cannot admit a format change. This
    // tests constraints only; the fixture backup remains non-off-host/creating.
    for registration in [
        None,
        Some(json!({})),
        Some(json!({"expected":{"backup":f.backup,"deployment":deployment}})),
    ] {
        let invalid = c.transaction().await.unwrap();
        assert!(invalid.execute("UPDATE volume_schema_jobs SET kind='upgrade',source_schema='0',backup_id=$2,backup_registration=$3 WHERE operation_id=$1", &[&operation,&f.backup,&registration]).await.is_err(), "missing registration fields must not pass SQL CHECK as UNKNOWN");
        invalid.rollback().await.unwrap();
    }
    let registration = json!({"credential":ctx.credential,"subject":ctx.subject,"expected":{"backup":f.backup,"deployment":deployment,"installation":{"manifest":{"data_schema_version":"0","plugin_id":manifest["plugin_id"],"publisher_id":manifest["publisher_id"]}}}});
    for field in [
        "credential",
        "subject",
        "backup",
        "deployment",
        "data_schema_version",
        "plugin_id",
        "publisher_id",
    ] {
        let mut wrong = registration.clone();
        match field {
            "credential" | "subject" => wrong[field] = json!(Uuid::new_v4()),
            "backup" | "deployment" => wrong["expected"][field] = json!(Uuid::new_v4()),
            "publisher_id" => {
                wrong["expected"]["installation"]["manifest"][field] = json!(Uuid::new_v4())
            }
            _ => wrong["expected"]["installation"]["manifest"][field] = json!("different"),
        }
        let invalid = c.transaction().await.unwrap();
        assert!(invalid.execute("UPDATE volume_schema_jobs SET kind='upgrade',source_schema='0',backup_id=$2,backup_registration=$3 WHERE operation_id=$1", &[&operation,&f.backup,&wrong]).await.is_err(), "original backup {field} binding is required");
        invalid.rollback().await.unwrap();
    }
    let stable = Plan {
        id: Uuid::new_v4(),
        digest: crate::contracts::digest(b"metadata plan only"),
        requires_exclusive_write: true,
        backward_read_compatible: false,
        reversible: false,
        temporary_bytes_required: 3145728,
    };
    for (state, with_plan, source_schema, kind) in [
        ("planned", false, "none", "bootstrap"),
        ("initializing", false, "none", "bootstrap"),
        ("applying", true, "none", "bootstrap"),
        ("planned", true, "1", "bootstrap"),
        ("planned", true, "none", "upgrade"),
    ] {
        let invalid = c.transaction().await.unwrap();
        let saved = with_plan.then(|| json!(stable));
        assert!(invalid.execute("UPDATE volume_schema_jobs SET state=$2,plan=$3,volume_fence=7,source_schema=$4,kind=$5 WHERE operation_id=$1", &[&operation,&state,&saved,&source_schema,&kind]).await.is_err(),"unconfirmed writes and non-backed upgrades must be rejected by durable constraints");
        invalid.rollback().await.unwrap();
    }
    c.execute("UPDATE volume_schema_jobs SET state='initializing',plan=$2,volume_fence=7 WHERE operation_id=$1", &[&operation,&json!(stable)]).await.unwrap();
    let resumed = job(app, operation).await.unwrap();
    assert_eq!(plan(&resumed).unwrap(), stable);
    assert_eq!(resumed.get::<_, Uuid>("instance_id"), instance);
    assert_eq!(
        context(&resumed, "initialize").idempotency_key,
        format!("{operation}:initialize")
    );
    assert_eq!(resumed.get::<_, String>("state"), "initializing");
    // Revoking only volume authority leaves deployment.enable intact. An
    // exact cached Operation still needs its original source AND scratch scope.
    let revoked = c.transaction().await.unwrap();
    db::auth_lock(&revoked, true).await.unwrap();
    revoked
        .execute(
            "UPDATE grants SET status='revoked' WHERE subject_id=$1 AND scope->>'kind'='volume'",
            &[&ctx.subject],
        )
        .await
        .unwrap();
    auth::authorize(
        &revoked,
        &ctx,
        "plugin.enable",
        &Resource::new("deployment", deployment, Some(ctx.subject)),
    )
    .await
    .unwrap();
    assert!(authorize_replay(&revoked, &ctx, operation).await.is_err());
    revoked.rollback().await.unwrap();
    let fresh = app
        .auth
        .login(
            &c,
            ctx.subject,
            "correct horse backup fixture password".into(),
            ctx.peer,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    let fresh = app
        .auth
        .authenticate(
            &c,
            fresh["access_token"].as_str().unwrap(),
            ctx.peer,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    assert_eq!(fresh.subject, ctx.subject);
    assert!(
        authorize_replay(&c, &fresh, operation).await.is_err(),
        "a new login cannot replace the original job credential"
    );
    authorize_replay(&c, &ctx, operation).await.unwrap();
    // No Verified binding or fabricated available/off-host backup was used.
    assert_eq!(
        c.query_one(
            "SELECT schema_state FROM volume_allocations WHERE volume_id=$1",
            &[&source]
        )
        .await
        .unwrap()
        .get::<_, String>(0),
        "unversioned"
    );
}
