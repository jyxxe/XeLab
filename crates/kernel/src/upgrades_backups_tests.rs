//! PostgreSQL policy/registration checks only. No fixture below is a verified
//! remote backup, stopped writer, physical format or successful migration.
use super::*;
use crate::backups::tests::Fixture;

#[tokio::test]
async fn actual_postgres_format_backup_rejects_incomplete_and_non_off_host_registration() {
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
    let deployment: Uuid = c
        .query_one(
            "SELECT deployment_id FROM backups WHERE id=$1",
            &[&f.backup],
        )
        .await
        .unwrap()
        .get(0);
    for state in ["creating", "ready", "failed", "expired"] {
        let tx = c.transaction().await.unwrap();
        tx.execute(
            "UPDATE backups SET state=$2 WHERE id=$1",
            &[&f.backup, &state],
        )
        .await
        .unwrap();
        tx.execute(
            "UPDATE storage_recovery_jobs SET state='succeeded' WHERE backup_id=$1",
            &[&f.backup],
        )
        .await
        .unwrap();
        let error = match Registration::capture(&tx, app, &ctx, f.backup, deployment, &json!({}))
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("local or incomplete registration cannot construct a migration backup"),
        };
        assert_eq!(
            error.message,
            "Format migration requires an available off-host deployment backup"
        );
        assert!(!tx
            .query_one("SELECT off_host FROM backups WHERE id=$1", &[&f.backup])
            .await
            .unwrap()
            .get::<_, bool>(0));
        tx.rollback().await.unwrap();
    }
    assert!(
        Registration::capture(&c, app, &ctx, Uuid::new_v4(), deployment, &json!({}))
            .await
            .is_err()
    );
    assert_eq!(
        c.query_one("SELECT count(*) FROM volume_schema_jobs", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    assert_eq!(
        c.query_one("SELECT count(*) FROM deployment_upgrade_jobs", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn actual_postgres_format_backup_keeps_historical_volume_and_original_grant_authority() {
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
    let volume_ids: Vec<Uuid> = c
        .query("SELECT id FROM volumes ORDER BY id", &[])
        .await
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect();
    assert_eq!(volume_ids.len(), 2);
    // This synthetic value exercises ONLY authorize(), never remote evidence.
    let registration = Registration {
        expected: Expected {
            repository: Uuid::new_v4(),
            backup: f.backup,
            deployment: Uuid::new_v4(),
            manifest_object: Uuid::new_v4(),
            manifest_digest: digest(b"policy fixture"),
            installation: json!({}),
            manifest: json!({}),
        },
        credential: ctx.credential,
        subject: ctx.subject,
        backup_operation: Uuid::new_v4(),
        backup_owner: ctx.subject,
        object_reference: "fixture-only-no-remote-evidence".into(),
        master_key_id: digest(b"fixture only"),
        volume_ids: volume_ids.clone(),
    };
    let paths = Policy::new(&c, ctx.peer)
        .credential(ctx.credential, 0)
        .await
        .unwrap();
    registration
        .authorize(&c, &ctx, Some(&paths))
        .await
        .unwrap();
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
    assert!(
        registration
            .authorize_replay(&c, &fresh, &paths)
            .await
            .is_err(),
        "a new valid login cannot take over the original registration"
    );
    for omitted in ["backup.read", "volume.read"] {
        let narrowed: Vec<_> = paths
            .iter()
            .filter(|path| path.rule["action"] != omitted)
            .cloned()
            .collect();
        assert!(
            registration
                .authorize(&c, &ctx, Some(&narrowed))
                .await
                .is_err(),
            "{omitted} remains required"
        );
    }
    let mut one_volume = paths.clone();
    for path in &mut one_volume {
        if path.rule["action"] == "volume.read" {
            path.rule["resource"] = json!({"kind":"volume","match":"ids","ids":[volume_ids[0]]});
        }
    }
    assert!(
        registration
            .authorize(&c, &ctx, Some(&one_volume))
            .await
            .is_err(),
        "current broad grants cannot widen the original historical UUID selection"
    );
    c.execute(
        "UPDATE volumes SET state='deleted' WHERE id=ANY($1)",
        &[&volume_ids],
    )
    .await
    .unwrap();
    registration
        .authorize(&c, &ctx, Some(&paths))
        .await
        .unwrap();
    assert_eq!(
        c.query_one("SELECT count(*) FROM volume_allocations", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0,
        "archived UUID authorization does not require old physical allocations"
    );
    let revoked = c.transaction().await.unwrap();
    db::auth_lock(&revoked, true).await.unwrap();
    revoked
        .execute(
            "UPDATE grants SET status='revoked' WHERE subject_id=$1",
            &[&ctx.subject],
        )
        .await
        .unwrap();
    assert!(registration
        .authorize(&revoked, &ctx, Some(&paths))
        .await
        .is_err());
    // Replacement bootstrap grant is metadata-only test setup. It proves that
    // current rights alone cannot revive an operation's withdrawn source path.
    revoked.execute("INSERT INTO grants(id,subject_id,issuer_id,issuer_credential_id,role_id,group_id,group_version,scope,expires_at,status,created_at,updated_at) SELECT gen_random_uuid(),subject_id,issuer_id,issuer_credential_id,role_id,group_id,group_version,scope,expires_at,'active',created_at,now() FROM grants WHERE subject_id=$1 AND role_id='Owner'", &[&ctx.subject]).await.unwrap();
    registration.authorize(&revoked, &ctx, None).await.unwrap();
    assert!(
        registration
            .authorize(&revoked, &ctx, Some(&paths))
            .await
            .is_err(),
        "a later grant cannot replace original authority"
    );
    revoked.rollback().await.unwrap();
    let blocked = c.transaction().await.unwrap();
    db::auth_lock(&blocked, true).await.unwrap();
    blocked
        .execute(
            "UPDATE credentials SET status='revoked' WHERE id=$1",
            &[&ctx.credential],
        )
        .await
        .unwrap();
    assert!(registration
        .authorize(&blocked, &ctx, Some(&paths))
        .await
        .is_err());
    blocked.rollback().await.unwrap();
}
