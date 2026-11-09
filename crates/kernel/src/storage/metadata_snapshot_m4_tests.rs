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
    let (ticket, request_binding, unfinished) =
        archived_host_handles(&source, &ctx, installation).await;
    assert!(crate::recovery::ensure_m4_authority_reset(&source)
        .await
        .is_err());
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
        "INSERT INTO outbox(id,topic,aggregate_id,payload,created_at) VALUES(gen_random_uuid(),'kernel.event.publish',gen_random_uuid(),'{}',clock_timestamp())",
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
    crate::recovery::ensure_m4_authority_reset(&destination)
        .await
        .unwrap();
    assert_eq!(
        destination
            .query_one("SELECT state FROM calls WHERE id=$1", &[&unfinished])
            .await
            .unwrap()
            .get::<_, String>(0),
        "unknown"
    );
    for (query, id) in [
        ("UPDATE host_call_tickets SET expires_at=clock_timestamp()+interval '1 minute' WHERE id=$1", ticket),
        ("UPDATE dependency_bindings SET expires_at=clock_timestamp()+interval '1 minute' WHERE id=$1", request_binding),
    ] {
        let tx = destination.transaction().await.unwrap();
        tx.execute(query, &[&id]).await.unwrap();
        assert!(crate::recovery::ensure_m4_authority_reset(&tx).await.is_err());
        tx.rollback().await.unwrap();
    }
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

// Unfinished crash metadata is deliberately inert: the instance is stopped and
// no execution delegation is minted. COPY must still fence every stored handle.
async fn archived_host_handles(
    c: &Client,
    ctx: &crate::auth::Context,
    installation: Uuid,
) -> (Uuid, Uuid, Uuid) {
    let deployment: Uuid = c
        .query_one(
            "SELECT id FROM deployments WHERE installation_id=$1",
            &[&installation],
        )
        .await
        .unwrap()
        .get(0);
    let instance = Uuid::new_v4();
    let call = Uuid::new_v4();
    let binding = Uuid::new_v4();
    let ticket = Uuid::new_v4();
    let digest = contracts::digest(b"archived M4 metadata fixture");
    let contract =
        json!({"name":"org.xelab.snapshot.archive","version":"1.0.0","schema_digest":digest});
    c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,created_at,updated_at) VALUES($1,$2,1,0,'stopped','unknown',$3,now(),now())", &[&instance,&deployment,&json!({"cpu_millis":100,"memory_bytes":67108864,"ephemeral_bytes":1048576,"pids":64})]).await.unwrap();
    c.execute("INSERT INTO calls(id,owner_id,credential_id,provider_instance_id,contract,input_digest,state,deadline_at,created_at) VALUES($1,$2,$3,$4,$5,$6,'running',now()+interval '5 minutes',now())", &[&call,&ctx.subject,&ctx.credential,&instance,&contract,&digest]).await.unwrap();
    c.execute("INSERT INTO dependency_bindings(id,consumer_deployment_id,alias,provider_deployment_id,contract,binding_scope,required,bound_call,expires_at) VALUES($1,$2,'inert-archived-request',$2,$3,'request',false,$4,now()+interval '5 minutes')", &[&binding,&deployment,&contract,&call]).await.unwrap();
    c.execute("INSERT INTO host_call_tickets(id,parent_call_id,instance_id,generation,delegation_digest,verification_digest,rpc_context,invocation,bounds,idempotency_key,fingerprint,input_digest,dependency_binding_id,expires_at) VALUES($1,$2,$3,1,$4,$5,'{}','{}','[]','archived-m4-ticket',$6,$6,$7,now()+interval '5 minutes')", &[&ticket,&call,&instance,&vec![1u8;32],&vec![2u8;32],&digest,&binding]).await.unwrap();
    (ticket, binding, call)
}
