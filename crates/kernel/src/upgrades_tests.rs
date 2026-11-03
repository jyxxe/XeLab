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
