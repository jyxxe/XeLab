//! Actual mTLS HostService event transport and real PostgreSQL; workload rows
//! are component fixtures, separate from privileged OCI/production acceptance.
use super::*;
use crate::{db, events};

async fn event_proxy(f: &Fixture, deployment: Uuid) -> (String, auth::Context) {
    let mut c = f.app.db.connect().await.unwrap();
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, true).await.unwrap();
    let ctx = f
        .app
        .auth
        .authenticate(&tx, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    let row=tx.query_one("SELECT t.id,t.manifest FROM deployments d JOIN installations t ON t.id=d.installation_id WHERE d.id=$1",&[&deployment]).await.unwrap();
    let installation: Uuid = row.get("id");
    let mut manifest: Value = row.get("manifest");
    let topic = format!("{}.completed", manifest["plugin_id"].as_str().unwrap());
    let schema = json!({"type":"object","properties":{"value":{"type":"integer"}},"required":["value"],"additionalProperties":false});
    manifest["event_topics"] = json!([{"topic":topic,"version":"1.0.0","payload_schema":schema,"schema_digest":crate::skills::definition_digest(&schema).unwrap()}]);
    let rules = json!([{"action":"blob.read","resource":{"kind":"blob","match":"all"}},{"action":"blob.create","resource":{"kind":"blob","match":"all"}},{"action":"event.publish","resource":{"kind":"event","match":"all"}},{"action":"event.subscribe","resource":{"kind":"event","match":"all"}}]);
    manifest["requested_rules"] = json!(rules
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| json!({"required":false,"rule":rule}))
        .collect::<Vec<_>>());
    crate::artifact::validate_profile(&manifest).unwrap();
    tx.execute(
        "UPDATE installations SET manifest=$2 WHERE id=$1",
        &[&installation, &manifest],
    )
    .await
    .unwrap();
    events::store(&tx, installation, &manifest).await.unwrap();
    let approval = json!({"rules":rules,"egress":[]});
    crate::host_blobs::approve(&tx, &ctx, deployment, &manifest, &approval, ctx.subject)
        .await
        .unwrap();
    tx.execute(
        "UPDATE deployments SET approval=$2 WHERE id=$1",
        &[&deployment, &approval],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (topic, ctx)
}

#[tokio::test]
async fn host_event_mtls_publication_dedupe_namespace_and_live_parent_fences() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (deployment, instance) = f.workload().await;
    let (topic, _) = event_proxy(&f, deployment).await;
    let source = f.source(b"event-source".to_vec()).await;
    let (call, mut context) = f.delegation(deployment, instance, source).await;
    context.idempotency_key = "publish-event".into();
    let server = tokio::spawn(crate::host::serve(f.app.clone()));
    let mut client = f
        .client(f.client_identity(&format!(
            "spiffe://xelab.local/instance/{instance}/generation/7"
        )))
        .await;
    let request = rpc::PublishEventRequest {
        context: Some(context.clone()),
        topic: topic.clone(),
        version: "1.0.0".into(),
        payload_json: serde_json::to_vec(&json!({"value":7})).unwrap(),
        dedupe_key: "event-one".into(),
    };
    let first = client
        .publish_event(request.clone())
        .await
        .unwrap()
        .into_inner();
    assert!(!first.duplicate);
    let second = client
        .publish_event(request.clone())
        .await
        .unwrap()
        .into_inner();
    assert!(second.duplicate);
    assert_eq!(first.event_id, second.event_id);
    let mut changed = request.clone();
    changed.payload_json = serde_json::to_vec(&json!({"value":8})).unwrap();
    assert_eq!(
        client.publish_event(changed).await.unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    let mut cross = request.clone();
    cross.topic = "org.xelab.some-other-plugin.completed".into();
    cross.context.as_mut().unwrap().idempotency_key = "cross-namespace".into();
    assert_eq!(
        client.publish_event(cross).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    let mut forged = request.clone();
    forged.context.as_mut().unwrap().actor_instance_id = Uuid::new_v4().to_string();
    assert_eq!(
        client.publish_event(forged).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    let mut old = f
        .client(f.client_identity(&format!(
            "spiffe://xelab.local/instance/{instance}/generation/6"
        )))
        .await;
    assert_eq!(
        old.publish_event(request.clone()).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    let c = f.app.db.connect().await.unwrap();
    c.execute("UPDATE calls SET state='succeeded' WHERE id=$1", &[&call])
        .await
        .unwrap();
    assert_eq!(
        client.publish_event(request).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    let count: i64 = c
        .query_one("SELECT count(*) FROM events", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);
    server.abort();
}

#[tokio::test]
async fn host_event_mtls_subscription_redelivery_ack_and_revocation() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (deployment, instance) = f.workload().await;
    let (topic, owner) = event_proxy(&f, deployment).await;
    let mut c = f.app.db.connect().await.unwrap();
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, false).await.unwrap();
    let (subscription,_)=events::execute(&tx,&f.app,&owner,"createSubscription",&std::collections::HashMap::new(),&json!({"topic":topic,"name":"Host event consumer","start":"earliest","maximum_attempts":5}),&axum::http::HeaderMap::new(),"").await.unwrap();
    let subscription: Uuid = subscription["id"].as_str().unwrap().parse().unwrap();
    tx.commit().await.unwrap();
    let source = f.source(b"event-subscribe-source".to_vec()).await;
    let (_, mut context) = f.delegation(deployment, instance, source).await;
    context.idempotency_key = "host-event-publish".into();
    let server = tokio::spawn(crate::host::serve(f.app.clone()));
    let mut client = f
        .client(f.client_identity(&format!(
            "spiffe://xelab.local/instance/{instance}/generation/7"
        )))
        .await;
    let accepted = client
        .publish_event(rpc::PublishEventRequest {
            context: Some(context.clone()),
            topic,
            version: "1.0.0".into(),
            payload_json: serde_json::to_vec(&json!({"value":3})).unwrap(),
            dedupe_key: "host-delivery".into(),
        })
        .await
        .unwrap()
        .into_inner();
    let tx = c.transaction().await.unwrap();
    events::materialize(&tx).await.unwrap();
    tx.commit().await.unwrap();
    let mut stream = client
        .subscribe(rpc::SubscribeRequest {
            context: Some(context.clone()),
            subscription_id: subscription.to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    let first = tokio::time::timeout(Duration::from_secs(5), stream.message())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first.event_id, accepted.event_id);
    assert_eq!(first.attempt, 1);
    let id: Uuid = first.delivery_id.parse().unwrap();
    c.execute("UPDATE event_deliveries SET lease_expires_at=clock_timestamp()-interval '60 seconds' WHERE id=$1",&[&id]).await.unwrap();
    let second = tokio::time::timeout(Duration::from_secs(5), stream.message())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(second.event_id, first.event_id);
    assert_eq!(second.attempt, 2);
    assert_ne!(second.delivery_id, first.delivery_id);
    assert!(
        second.lease_token != first.lease_token,
        "redelivery must rotate the lease secret"
    );
    let mut ack_context = context.clone();
    ack_context.idempotency_key = "stale-host-event-ack".into();
    let stale = rpc::AckEventRequest {
        context: Some(ack_context),
        subscription_id: subscription.to_string(),
        delivery_id: first.delivery_id,
        lease_token: first.lease_token,
        outcome: 1,
    };
    assert_eq!(
        client.ack_event(stale).await.unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    let mut ack_context = context.clone();
    ack_context.idempotency_key = "valid-host-event-ack".into();
    let ack = rpc::AckEventRequest {
        context: Some(ack_context),
        subscription_id: subscription.to_string(),
        delivery_id: second.delivery_id,
        lease_token: second.lease_token,
        outcome: 1,
    };
    assert!(
        client
            .ack_event(ack.clone())
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    assert!(
        client
            .ack_event(ack.clone())
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    c.execute(
        "UPDATE credentials SET status='revoked' WHERE id=$1",
        &[&owner.credential],
    )
    .await
    .unwrap();
    assert_eq!(
        client.ack_event(ack).await.unwrap_err().code(),
        tonic::Code::Unauthenticated
    );
    let stopped = tokio::time::timeout(Duration::from_secs(5), stream.message())
        .await
        .unwrap();
    assert!(
        stopped.is_err() || stopped.unwrap().is_none(),
        "revocation must stop the retained subscription stream"
    );
    server.abort();
}

#[tokio::test]
async fn host_event_actual_parent_expiry_fences_a_blocked_publication() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (deployment, instance) = f.workload().await;
    let (topic, _) = event_proxy(&f, deployment).await;
    let source = f.source(b"event-expiry-source".to_vec()).await;
    let (call, mut context) = f.delegation(deployment, instance, source).await;
    context.idempotency_key = "expired-event-publication".into();
    let mut held = f.app.db.connect().await.unwrap();
    let held = held.transaction().await.unwrap();
    events::fence(&held).await.unwrap();
    let c = f.app.db.connect().await.unwrap();
    c.execute(
        "UPDATE calls SET deadline_at=clock_timestamp()+interval '3 seconds' WHERE id=$1",
        &[&call],
    )
    .await
    .unwrap();
    // The body deliberately claims the original longer deadline. The server
    // must stop at the live parent authority's earlier deadline.
    let request = rpc::PublishEventRequest {
        context: Some(context),
        topic,
        version: "1.0.0".into(),
        payload_json: serde_json::to_vec(&json!({"value":1})).unwrap(),
        dedupe_key: "authority-expiry".into(),
    };
    let app = f.app.clone();
    let pending = tokio::spawn(async move {
        crate::host_events::publish(
            &app,
            crate::host_calls::Workload {
                instance,
                generation: 7,
            },
            request,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2),async {
        loop {
            let blocked:bool=c.query_one("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%event_store_limits%')",&[]).await.unwrap().get(0);
            if blocked {break;}
            assert!(!pending.is_finished(),"the source must reach the contended storage fence before expiry");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("event publication did not reach the storage fence");
    let error = tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .expect("actual parent expiry did not stop the blocked publication")
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "DEADLINE_EXCEEDED");
    held.rollback().await.unwrap();
    let count: i64 = c
        .query_one(
            "SELECT count(*) FROM events WHERE dedupe_key='authority-expiry'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        count, 0,
        "expired authority must not commit a delayed event"
    );
}
