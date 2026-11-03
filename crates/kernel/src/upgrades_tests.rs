//! Real PostgreSQL admission. Installation and capacity rows are fixtures;
//! this is not signed OCI, actual migration or production host acceptance.
use super::*;
use crate::{backups::tests::Fixture, contracts::digest};

#[tokio::test]
async fn actual_postgres_upgrade_admission_is_atomic_and_keeps_original_authority() {
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
    let mut manifest: Value =
        serde_json::from_str(include_str!("../tests/fixtures/secret-manifest.json")).unwrap();
    manifest["requested_rules"] = json!([]);
    manifest["publisher_id"] = json!(publisher);
    let source = Uuid::new_v4();
    let target = Uuid::new_v4();
    for (id, version) in [(source, "1.0.0"), (target, "2.0.0")] {
        manifest["version"] = json!(version);
        let hash = digest(&serde_json::to_vec(&manifest).unwrap());
        c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,'org.xelab.echo-python',$2,$3,$4,$4,$5,'installed',now(),now())",&[&id,&version,&publisher,&hash,&manifest]).await.unwrap();
    }
    let deployment = Uuid::new_v4();
    let spec = json!({"installation_id":source,"display_name":"upgrade admission fixture","enabled":true,"replicas":1,"limits":manifest["minimum_resources"],"config":{},"volume_ids":[]});
    c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,$4,'ready',now(),now())",&[&deployment,&ctx.subject,&source,&spec]).await.unwrap();
    let body = json!({"installation_id":target,"strategy":"recreate","allow_downtime":true});
    let mut headers = HeaderMap::new();
    headers.insert("if-match", "\"rev-1\"".parse().unwrap());
    let old: i64 = c
        .query_one("SELECT count(*) FROM deployment_upgrade_jobs", &[])
        .await
        .unwrap()
        .get(0);
    for variant in ["etag", "downtime", "parallel", "backup", "forward_rollback"] {
        let tx = c.transaction().await.unwrap();
        db::auth_lock(&tx, false).await.unwrap();
        let mut wrong = body.clone();
        let mut h = headers.clone();
        match variant {
            "etag" => {
                h.insert("if-match", "\"rev-2\"".parse().unwrap());
            }
            "downtime" => wrong["allow_downtime"] = json!(false),
            "parallel" => wrong["strategy"] = json!("parallel_compatible"),
            "backup" => wrong["backup_id"] = json!(f.backup),
            _ => {}
        };
        let action = if variant == "forward_rollback" {
            "plugin.rollback"
        } else {
            "plugin.upgrade"
        };
        assert!(create(&tx, app, &ctx, deployment, action, &wrong, &h)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        assert_eq!(
            c.query_one("SELECT count(*) FROM deployment_upgrade_jobs", &[])
                .await
                .unwrap()
                .get::<_, i64>(0),
            old
        );
        assert_eq!(
            c.query_one(
                "SELECT count(*) FROM instances WHERE deployment_id=$1",
                &[&deployment]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
            0
        );
    }
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, false).await.unwrap();
    let (result, status) = create(
        &tx,
        app,
        &ctx,
        deployment,
        "plugin.upgrade",
        &body,
        &headers,
    )
    .await
    .unwrap();
    assert_eq!(status, 202);
    let operation = id(&result, "id").unwrap();
    tx.commit().await.unwrap();
    let row=c.query_one("SELECT j.*,host(j.peer) AS approved_peer FROM deployment_upgrade_jobs j WHERE operation_id=$1",&[&operation]).await.unwrap();
    assert_eq!(row.get::<_, String>("state"), "freezing");
    assert_eq!(row.get::<_, Value>("original_spec"), spec);
    assert_eq!(
        row.get::<_, Value>("target_spec")["installation_id"],
        json!(target)
    );
    let candidate: Uuid = row.get("candidate_id");
    assert_eq!(
        c.query_one("SELECT phase FROM instances WHERE id=$1", &[&candidate])
            .await
            .unwrap()
            .get::<_, String>(0),
        "pending"
    );
    assert_eq!(
        c.query_one(
            "SELECT installation_id FROM deployments WHERE id=$1",
            &[&deployment]
        )
        .await
        .unwrap()
        .get::<_, Uuid>(0),
        source
    );
    assert_eq!(
        revalidate(&c, app, &row).await.unwrap().subject,
        ctx.subject
    );
    authorize(&c, &ctx, deployment, "plugin.upgrade", &body)
        .await
        .unwrap();
    // Mapping/permission checks use real FK-constrained PostgreSQL rows. These
    // are metadata fixtures, not physical allocation or runtime stop proofs.
    let mapped = c.transaction().await.unwrap();
    let root = Uuid::new_v4();
    let old_volume = Uuid::new_v4();
    let new_volume = Uuid::new_v4();
    let volume_spec = json!({"name":"data","capacity_bytes":1048576,"access":"single_writer","scope":"deployment"});
    mapped.execute("INSERT INTO volume_storage_root(id,filesystem_uuid,state,capacity_bytes) VALUES($1,$2,'ready',67108864)", &[&root,&Uuid::new_v4()]).await.unwrap();
    for (volume, state) in [(old_volume, "ready"), (new_volume, "restoring")] {
        mapped.execute("INSERT INTO volume_allocations(volume_id,root_id,project_id,owner_id,spec,capacity_bytes,inode_limit,metadata_charge,desired_capacity,desired_inodes,desired_metadata_charge,state,operation_id,credential_id,peer) VALUES($1,$2,nextval('volume_project_ids'),$3,$4,1048576,64,65536,1048576,64,65536,$5,$6,$7,'127.0.0.1')", &[&volume,&root,&ctx.subject,&volume_spec,&state,&operation,&ctx.credential]).await.unwrap();
    }
    mapped.execute("INSERT INTO volumes(id,owner_id,spec,backend_id,capacity_bytes,write_fence,state,created_at,updated_at) VALUES($1,$2,$3,$4,1048576,7,'available',now(),now())", &[&old_volume,&ctx.subject,&volume_spec,&old_volume.to_string()]).await.unwrap();
    mapped
        .execute(
            "INSERT INTO volume_holds(volume_id,operation_id,kind) VALUES($1,$2,'migration')",
            &[&old_volume, &operation],
        )
        .await
        .unwrap();
    mapped.execute("INSERT INTO deployment_upgrade_volumes(operation_id,ordinal,source_volume_id,target_volume_id,source_spec,state) VALUES($1,0,$2,$3,$4,'reserved')", &[&operation,&old_volume,&new_volume,&volume_spec]).await.unwrap();
    mapped.execute("UPDATE deployment_upgrade_jobs SET original_spec=jsonb_set(original_spec,'{volume_ids}',$2),target_spec=jsonb_set(target_spec,'{volume_ids}',$3) WHERE operation_id=$1", &[&operation,&json!([old_volume]),&json!([new_volume])]).await.unwrap();
    let mapping_job = mapped
        .query_one(
            "SELECT * FROM deployment_upgrade_jobs WHERE operation_id=$1",
            &[&operation],
        )
        .await
        .unwrap();
    for permitted in [
        vec![old_volume],
        vec![new_volume],
        vec![old_volume, new_volume],
    ] {
        let rules = json!([
            {"action":"volume.create","resource":{"kind":"volume","match":"owned_by","owner_id":ctx.subject}},
            {"action":"volume.read","resource":{"kind":"volume","match":"ids","ids":permitted}}
        ]);
        assert_eq!(
            volume_group::authorize_saved(&mapped, &ctx, &mapping_job, &rules)
                .await
                .is_ok(),
            permitted.len() == 2,
            "both the retained source and new private UUID require authority"
        );
    }
    volume_group::freeze(&mapped, operation).await.unwrap();
    let fences = mapped.query_one("SELECT source_fence,target_fence FROM deployment_upgrade_volumes WHERE operation_id=$1", &[&operation]).await.unwrap();
    assert_eq!(fences.get::<_, Option<i64>>(0), Some(8));
    assert_eq!(fences.get::<_, Option<i64>>(1), Some(9));
    volume_group::failed(&mapped, operation).await.unwrap();
    assert_eq!(
        mapped
            .query_one(
                "SELECT state FROM volume_allocations WHERE volume_id=$1",
                &[&new_volume]
            )
            .await
            .unwrap()
            .get::<_, String>(0),
        "quarantined"
    );
    assert_eq!(
        mapped
            .query_one(
                "SELECT state FROM volume_allocations WHERE volume_id=$1",
                &[&old_volume]
            )
            .await
            .unwrap()
            .get::<_, String>(0),
        "ready"
    );
    assert!(mapped
        .query_opt("SELECT id FROM volumes WHERE id=$1", &[&new_volume])
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        mapped
            .query_one(
                "SELECT count(*) FROM volume_holds WHERE operation_id=$1",
                &[&operation]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    mapped.rollback().await.unwrap();
    let revoked = c.transaction().await.unwrap();
    db::auth_lock(&revoked, true).await.unwrap();
    revoked
        .execute(
            "UPDATE grants SET status='revoked' WHERE subject_id=$1 AND scope->>'kind'='plugin'",
            &[&ctx.subject],
        )
        .await
        .unwrap();
    auth::authorize(
        &revoked,
        &ctx,
        "plugin.upgrade",
        &Resource::new("deployment", deployment, Some(ctx.subject)),
    )
    .await
    .unwrap();
    assert!(
        authorize(&revoked, &ctx, deployment, "plugin.upgrade", &body)
            .await
            .is_err(),
        "replay must still check target plugin.read, even when deployment upgrade stays allowed"
    );
    assert!(revalidate(&revoked, app, &row).await.is_err());
    revoked.rollback().await.unwrap();
    // Stop proofs below are metadata fixtures. The separate OCI drill verifies
    // actual process absence before invoking this same recovery transaction.
    for scenario in ["allowed", "revoked", "no_admission"] {
        let recovery = c.transaction().await.unwrap();
        db::auth_lock(&recovery, true).await.unwrap();
        recovery
            .query_one("SELECT id FROM node_budget FOR UPDATE", &[])
            .await
            .unwrap();
        recovery
            .execute(
                "UPDATE node_budget SET admission_enabled=$1",
                &[&(scenario != "no_admission")],
            )
            .await
            .unwrap();
        recovery
            .execute(
                "UPDATE instances SET phase='stopped' WHERE id=$1",
                &[&candidate],
            )
            .await
            .unwrap();
        if scenario == "revoked" {
            recovery
                .execute(
                    "UPDATE credentials SET status='revoked' WHERE id=$1",
                    &[&ctx.credential],
                )
                .await
                .unwrap();
        }
        let outcome = worker::recover_source(&recovery, app, &row).await.unwrap();
        match outcome {
            worker::SourceRecovery::Created(instance) => {
                assert_eq!(scenario, "allowed");
                assert_ne!(instance, candidate);
                let next = recovery
                    .query_one(
                        "SELECT generation,phase FROM instances WHERE id=$1",
                        &[&instance],
                    )
                    .await
                    .unwrap();
                assert!(next.get::<_, i64>(0) > row.get::<_, i64>("candidate_generation"));
                assert_eq!(next.get::<_, String>(1), "pending");
                assert_eq!(recovery.query_one("SELECT count(*) FROM reservations WHERE instance_id=$1 AND status='reserved'",&[&instance]).await.unwrap().get::<_,i64>(0),1);
                assert_eq!(
                    recovery
                        .query_one("SELECT spec FROM deployments WHERE id=$1", &[&deployment])
                        .await
                        .unwrap()
                        .get::<_, Value>(0),
                    spec
                );
            }
            worker::SourceRecovery::Blocked => {
                assert_ne!(scenario, "allowed");
                let disabled = recovery
                    .query_one(
                        "SELECT spec,phase FROM deployments WHERE id=$1",
                        &[&deployment],
                    )
                    .await
                    .unwrap();
                assert_eq!(disabled.get::<_, Value>(0)["enabled"], false);
                assert_eq!(disabled.get::<_, String>(1), "disabled");
                assert_eq!(
                    recovery
                        .query_one(
                            "SELECT count(*) FROM instances WHERE deployment_id=$1",
                            &[&deployment]
                        )
                        .await
                        .unwrap()
                        .get::<_, i64>(0),
                    1,
                    "a rejected restart must leave no new instance intent"
                );
                assert_eq!(
                    recovery
                        .query_one(
                            "SELECT count(*) FROM reservations WHERE instance_id=$1",
                            &[&candidate]
                        )
                        .await
                        .unwrap()
                        .get::<_, i64>(0),
                    0
                );
            }
            worker::SourceRecovery::NotRequired => {
                panic!("enabled unchanged source must be resumed or explicitly blocked")
            }
        }
        assert_eq!(
            recovery
                .query_one(
                    "SELECT installation_id FROM deployments WHERE id=$1",
                    &[&deployment]
                )
                .await
                .unwrap()
                .get::<_, Uuid>(0),
            source
        );
        recovery.rollback().await.unwrap();
    }
    let tx = c.transaction().await.unwrap();
    assert!(create(
        &tx,
        app,
        &ctx,
        deployment,
        "plugin.upgrade",
        &body,
        &headers
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    c.execute(
        "UPDATE publishers SET status='revoked' WHERE id=$1",
        &[&publisher],
    )
    .await
    .unwrap();
    assert!(revalidate(&c, app, &row).await.is_err());
    c.execute(
        "UPDATE publishers SET status='trusted' WHERE id=$1",
        &[&publisher],
    )
    .await
    .unwrap();
    revalidate(&c, app, &row).await.unwrap();
    let original_hash: String = c
        .query_one(
            "SELECT image_digest FROM installations WHERE id=$1",
            &[&target],
        )
        .await
        .unwrap()
        .get(0);
    c.execute(
        "UPDATE installations SET image_digest=$2 WHERE id=$1",
        &[&target, &digest(b"changed target")],
    )
    .await
    .unwrap();
    assert!(revalidate(&c, app, &row).await.is_err());
    c.execute(
        "UPDATE installations SET image_digest=$2 WHERE id=$1",
        &[&target, &original_hash],
    )
    .await
    .unwrap();
    c.execute(
        "UPDATE credentials SET status='revoked' WHERE id=$1",
        &[&ctx.credential],
    )
    .await
    .unwrap();
    assert!(revalidate(&c, app, &row).await.is_err());
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
    assert!(fresh["access_token"].is_string());
    assert!(
        revalidate(&c, app, &row).await.is_err(),
        "a new session cannot rescue the old intent's source"
    );
    assert!(c.query_opt("SELECT deployment_id FROM deployment_storage_holds WHERE deployment_id=$1 AND operation_id=$2",&[&deployment,&operation]).await.unwrap().is_some(),"authority loss must not release an unfenced runtime hold");
    assert_eq!(
        c.query_one("SELECT state FROM operations WHERE id=$1", &[&operation])
            .await
            .unwrap()
            .get::<_, String>(0),
        "pending"
    );
    let rules=c.query_one("SELECT condeferrable,condeferred FROM pg_constraint WHERE conname='deployment_storage_holds_operation_id_fkey'",&[]).await.unwrap();
    assert!(!rules.get::<_, bool>(0) && !rules.get::<_, bool>(1));
}
