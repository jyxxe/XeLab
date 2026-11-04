use super::*;

fn output() -> (Output, mpsc::Receiver<(Vec<u8>, bool, u64)>) {
    let (sender, receiver) = mpsc::channel(1);
    let (head, _) = oneshot::channel();
    (
        Output {
            sender,
            head: Mutex::new(Some(head)),
            identity: Arc::new(Mutex::new(None)),
            sequence: Mutex::new(0),
            stalled: std::sync::atomic::AtomicBool::new(false),
            terminal: std::sync::atomic::AtomicBool::new(false),
        },
        receiver,
    )
}
#[tokio::test]
async fn sequence_commits_only_after_bounded_encode_and_delivery_to_queue() {
    let (output, mut receiver) = output();
    let call = Uuid::new_v4();
    output.bind(call, chrono::Utc::now() + chrono::Duration::seconds(30));
    let accepted =
        json!({"call_id":call,"sequence":1,"kind":"accepted","payload":{},"terminal":false});
    output.emit(&accepted).await.unwrap();
    let oversized = json!({"call_id":call,"sequence":2,"kind":"item","payload":"x".repeat(1048576),"terminal":false});
    assert!(output.emit(&oversized).await.is_err());
    assert_eq!(*output.sequence.lock().unwrap(), 1);
    assert_eq!(receiver.recv().await.unwrap().2, 1);
    drop(receiver);
    assert!(output
        .emit(
            &json!({"call_id":call,"sequence":2,"kind":"item","payload":"normal","terminal":false})
        )
        .await
        .is_err());
    assert_eq!(*output.sequence.lock().unwrap(), 1);
}
#[tokio::test]
async fn retained_body_capacity_survives_producer_exit_and_eof_until_body_drop() {
    let pool = Arc::new(Semaphore::new(1));
    let permit = pool.clone().try_acquire_owned().unwrap();
    let body = retained_body(Body::from("done"), permit);
    let mut stream = body.into_data_stream();
    assert_eq!(stream.next().await.unwrap().unwrap().as_ref(), b"done");
    assert!(stream.next().await.is_none());
    assert!(pool.clone().try_acquire_owned().is_err());
    drop(stream);
    assert!(pool.try_acquire_owned().is_ok());
}
#[tokio::test]
async fn consumer_terminal_uses_its_next_sequence_even_if_producer_is_ahead() {
    let (output, _) = output();
    let call = Uuid::new_v4();
    output.bind(call, chrono::Utc::now() + chrono::Duration::seconds(30));
    *output.sequence.lock().unwrap() = 9;
    let (frame, terminal, sequence) = error_frame(&output.identity, Error::denied(), 2).unwrap();
    assert!(terminal);
    assert_eq!(sequence, 2);
    let frame = String::from_utf8(frame).unwrap();
    assert!(frame.contains(&format!("id: {call}:2\n")));
    assert!(frame.contains("\"sequence\":2"));
}
#[tokio::test]
async fn consumer_close_cancels_pending_admission_but_not_a_committing_journal() {
    let (output, receiver) = output();
    drop(receiver);
    tokio::time::timeout(Duration::from_secs(1), output.abandoned_before_admission())
        .await
        .unwrap();
    output.bind(
        Uuid::new_v4(),
        chrono::Utc::now() + chrono::Duration::seconds(30),
    );
    assert!(tokio::time::timeout(
        Duration::from_millis(10),
        output.abandoned_before_admission()
    )
    .await
    .is_err());
}

#[tokio::test]
async fn current_authority_cannot_cross_transactions_contexts_or_fixed_plugin_owners() {
    let Some(f) = crate::backups::tests::Fixture::new().await else {
        return;
    };
    let mut c = f.app.db.connect().await.unwrap();
    let tx = c.transaction().await.unwrap();
    let ctx = f
        .app
        .auth
        .authenticate(&tx, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    let mut proof = crate::auth::CurrentAuthority::new(&tx, &ctx).await.unwrap();
    let owned = Resource::new("plugin", "org.xelab.http-echo", Some(ctx.subject));
    proof.bind_plugin(&tx, &ctx, &owned).unwrap();
    proof
        .require(&tx, &ctx, "plugin.http.invoke", &owned)
        .unwrap();
    let foreign = Resource::new("plugin", "org.xelab.http-echo", Some(Uuid::new_v4()));
    assert!(proof
        .require(&tx, &ctx, "plugin.http.invoke", &foreign)
        .is_err());
    assert!(proof.bind_plugin(&tx, &ctx, &foreign).is_err());
    let mut second = f.app.db.connect().await.unwrap();
    let other = second.transaction().await.unwrap();
    assert!(proof.verify(&other, &ctx).is_err());
    let mut changed = ctx.clone();
    changed.credential = Uuid::new_v4();
    assert!(proof.paths(&tx, &changed, &owned).is_err());
    drop(proof);
    tx.rollback().await.unwrap();
    other.rollback().await.unwrap();
}

#[tokio::test]
async fn transaction_authority_does_not_cache_mutable_source_call_budgets() {
    let Some(f) = crate::backups::tests::Fixture::new().await else {
        return;
    };
    let mut c = f.app.db.connect().await.unwrap();
    let tx = c.transaction().await.unwrap();
    let ctx = f
        .app
        .auth
        .authenticate(&tx, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    tx.execute(
        "UPDATE credentials SET conditions='{\"max_calls\":1}' WHERE id=$1",
        &[&ctx.credential],
    )
    .await
    .unwrap();
    let mut proof = crate::auth::CurrentAuthority::new(&tx, &ctx).await.unwrap();
    let resource = Resource::new("plugin", "org.xelab.http-echo", Some(ctx.subject));
    proof.bind_plugin(&tx, &ctx, &resource).unwrap();
    let chosen = crate::identity::call_sources_from_paths(
        &tx,
        &ctx,
        "plugin.http.invoke",
        &resource,
        proof.paths(&tx, &ctx, &resource).unwrap().to_vec(),
    )
    .await
    .unwrap();
    assert!(chosen.contains(&ctx.credential));
    let call = Uuid::new_v4();
    let contract = json!({"name":"org.xelab.http-echo.http","version":"1.0.0","schema_digest":crate::contracts::digest(b"budget")});
    tx.execute("INSERT INTO calls(id,owner_id,credential_id,contract,input_digest,state,deadline_at,created_at) VALUES($1,$2,$3,$4,$5,'running',now()+interval '30 seconds',now())",&[&call,&ctx.subject,&ctx.credential,&contract,&crate::contracts::digest(b"input")]).await.unwrap();
    tx.execute(
        "INSERT INTO call_credential_charges(call_id,credential_id) VALUES($1,$2)",
        &[&call, &ctx.credential],
    )
    .await
    .unwrap();
    let error = crate::identity::call_sources_from_paths(
        &tx,
        &ctx,
        "plugin.http.invoke",
        &resource,
        proof.paths(&tx, &ctx, &resource).unwrap().to_vec(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "QUOTA_EXCEEDED");
    drop(proof);
    tx.rollback().await.unwrap();
}
