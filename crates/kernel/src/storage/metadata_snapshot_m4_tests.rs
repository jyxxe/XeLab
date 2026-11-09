//! Actual binary COPY preserves event data while discarding consumer authority.
use super::*;
use crate::{backups::tests::Fixture, contracts, events};
use axum::http::HeaderMap;
use serde_json::{json, Value};
use std::collections::HashMap;

#[tokio::test]
async fn actual_postgres_event_copy_preserves_pending_outbox_and_dead_letters_but_fences_old_leases(
) {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut source = f.app.db.connect().await.unwrap();
    let ctx = f
        .app
        .auth
        .authenticate(
            &source,
            &f.token,
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    let row = source
        .query_one(
            "SELECT id,manifest FROM installations WHERE plugin_id='org.xelab.backup-fixture'",
            &[],
        )
        .await
        .unwrap();
    let installation: Uuid = row.get("id");
    let mut manifest: Value = row.get("manifest");
    manifest["plugin_id"] = json!("org.xelab.backup-fixture");
    let topic = "org.xelab.backup-fixture.snapshot";
    let schema = json!({"type":"object","properties":{"n":{"type":"integer"}},"required":["n"],"additionalProperties":false});
    manifest["event_topics"] = json!([{"topic":topic,"version":"1.0.0","schema_digest":crate::skills::definition_digest(&schema).unwrap(),"payload_schema":schema}]);
    source
        .execute(
            "UPDATE installations SET manifest=$2 WHERE id=$1",
            &[&installation, &manifest],
        )
        .await
        .unwrap();
    let tx = source.transaction().await.unwrap();
    db::auth_lock(&tx, false).await.unwrap();
    events::store(&tx, installation, &manifest).await.unwrap();
    let first = events::append(
        &tx,
        &f.app.auth,
        &ctx,
        &json!({"topic":topic,"version":"1.0.0","payload":{"n":1},"dedupe_key":"snapshot-first"}),
    )
    .await
    .unwrap()
    .0;
    events::materialize(&tx).await.unwrap();
    let mut subscriptions = Vec::new();
    for name in ["snapshot leased", "snapshot dead letter"] {
        let sub = events::execute(
            &tx,
            &f.app,
            &ctx,
            "createSubscription",
            &HashMap::new(),
            &json!({"topic":topic,"name":name,"start":"earliest","maximum_attempts":1}),
            &HeaderMap::new(),
            "",
        )
        .await
        .unwrap()
        .0;
        subscriptions.push(contracts::id(&sub, "id").unwrap());
    }
    let leased = events::lease(&tx, &f.app.auth, &ctx, subscriptions[0])
        .await
        .unwrap()
        .unwrap();
    let dead = events::lease(&tx, &f.app.auth, &ctx, subscriptions[1])
        .await
        .unwrap()
        .unwrap();
    events::ack(&tx, &f.app.auth, &ctx, subscriptions[1], &json!({"delivery_id":dead["delivery_id"],"lease_token":dead["lease_token"],"outcome":"dead_letter"})).await.unwrap();
    let pending = events::append(&tx, &f.app.auth, &ctx, &json!({"topic":topic,"version":"1.0.0","payload":{"n":9007199254740993u64},"dedupe_key":"snapshot-pending"})).await.unwrap().0;
    tx.commit().await.unwrap();
    for mutation in [
        "UPDATE subscriptions SET cursor_sequence=(SELECT next_sequence FROM event_store_limits) WHERE cursor IS NULL",
        "UPDATE subscriptions SET spec=jsonb_set(spec,'{topic}','\"org.xelab.backup-fixture.wrong\"')",
        "UPDATE event_deliveries SET state='retry',retry_at=NULL WHERE state='leased'",
    ] {
        let tx = source.transaction().await.unwrap();
        tx.batch_execute(mutation).await.unwrap();
        assert!(events::snapshot::validate_snapshot(&tx).await.is_err(), "accepted corrupt event snapshot: {mutation}");
        tx.rollback().await.unwrap();
    }
    let mut bytes = Vec::new();
    let summary = export_inner(&mut source, &mut bytes).await.unwrap();
    let base = std::env::var("XELAB_TEST_DATABASE_URL").unwrap();
    let admin = db::Db::new(base.clone()).unwrap().connect().await.unwrap();
    let name = format!("event_copy_restore_{}", Uuid::new_v4().simple());
    admin
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let target = db::Db::new(format!("{base} dbname={name}")).unwrap();
    target.migrate().await.unwrap();
    let mut destination = target.connect().await.unwrap();
    let recovery = Uuid::new_v4();
    destination.execute("INSERT INTO platform_recovery_state(recovery_id,backup_id,phase) VALUES($1,$2,'importing')", &[&recovery,&f.backup]).await.unwrap();
    import_inner(
        &mut destination,
        &mut bytes.as_slice(),
        &summary,
        recovery,
        ctx.subject,
        "fresh M4 event snapshot recovery password",
    )
    .await
    .unwrap();
    events::snapshot::ensure_authority_reset(&destination)
        .await
        .unwrap();
    events::snapshot::validate_snapshot(&destination)
        .await
        .unwrap();
    let pending_id = contracts::id(&pending, "id").unwrap();
    assert!(!destination
        .query_one("SELECT published FROM events WHERE id=$1", &[&pending_id])
        .await
        .unwrap()
        .get::<_, bool>(0));
    assert_eq!(
        destination
            .query_one(
                "SELECT count(*) FROM event_dead_letters WHERE event_id=$1",
                &[&contracts::id(&first, "id").unwrap()]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    assert!(f
        .app
        .auth
        .authenticate(&destination, &f.token, ctx.peer, Uuid::new_v4())
        .await
        .is_err());
    let tx = destination.transaction().await.unwrap();
    db::auth_lock(&tx, false).await.unwrap();
    assert!(events::ack(&tx,&f.app.auth,&ctx,subscriptions[0],&json!({"delivery_id":leased["delivery_id"],"lease_token":leased["lease_token"],"outcome":"ack"})).await.is_err());
    tx.rollback().await.unwrap();
    assert_eq!(
        destination
            .query_one(
                "SELECT cursor_sequence FROM subscriptions WHERE id=$1",
                &[&subscriptions[0]]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    let tx = destination.transaction().await.unwrap();
    events::materialize(&tx).await.unwrap();
    events::materialize(&tx).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        destination
            .query_one("SELECT count(*) FROM events", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        2
    );
    destination
        .execute(
            "UPDATE subscriptions SET status='active' WHERE id=$1",
            &[&subscriptions[0]],
        )
        .await
        .unwrap();
    assert!(events::snapshot::ensure_authority_reset(&destination)
        .await
        .is_err());
    source
        .execute(
            "UPDATE event_dedupe SET payload_digest='sha256:bad' WHERE event_id=$1",
            &[&pending_id],
        )
        .await
        .unwrap();
    assert!(export_inner(&mut source, &mut Vec::new()).await.is_err());
}
