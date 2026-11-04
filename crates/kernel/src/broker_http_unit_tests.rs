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

async fn queued_advisory_lock(observer: &tokio_postgres::Client, pid: i32, mode: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let queued: bool = observer
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND mode=$2 AND NOT granted)",
                    &[&pid, &mode],
                )
                .await
                .unwrap()
                .get(0);
            if queued {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("The intended advisory lock waiter did not reach PostgreSQL");
}

async fn ledger_user(
    mut connection: crate::db::ScopedClient,
    occupied: Arc<std::sync::atomic::AtomicUsize>,
    name: &'static str,
) {
    use std::sync::atomic::Ordering;
    let tx = connection.transaction().await.unwrap();
    db::call_lock(&tx).await.unwrap();
    assert_eq!(
        occupied.fetch_add(1, Ordering::SeqCst),
        0,
        "{name} entered an occupied call ledger"
    );
    // Keep the actual PostgreSQL ledger lock through a scheduling opportunity
    // for the competing RPC/cancel transaction; no synthetic lock substitutes.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(occupied.fetch_sub(1, Ordering::SeqCst), 1);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn prepared_http_reader_crosses_queued_auth_writer_before_rpc_and_cancel_ledger_users() {
    let Some(f) = crate::backups::tests::Fixture::new().await else {
        return;
    };
    let timeout = Duration::from_secs(10);
    let mut prepared_connection = f.app.db.connect_scoped(timeout).await.unwrap();
    let prepared = prepared_connection.transaction().await.unwrap();
    db::auth_lock(&prepared, false).await.unwrap();

    let mut writer_connection = f.app.db.connect_scoped(timeout).await.unwrap();
    let writer_pid: i32 = writer_connection
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let (writer_acquired, acquired) = oneshot::channel();
    let (release_writer, released) = oneshot::channel();
    let writer = tokio::spawn(async move {
        let tx = writer_connection.transaction().await.unwrap();
        db::auth_lock(&tx, true).await.unwrap();
        writer_acquired.send(()).unwrap();
        released.await.unwrap();
        tx.commit().await.unwrap();
    });
    let observer = f.app.db.connect_scoped(timeout).await.unwrap();
    queued_advisory_lock(&observer, writer_pid, "ExclusiveLock").await;

    let rpc_connection = f.app.db.connect_scoped(timeout).await.unwrap();
    let rpc_pid: i32 = rpc_connection
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let occupied = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rpc = tokio::spawn(ledger_user(rpc_connection, occupied.clone(), "RPC"));
    queued_advisory_lock(&observer, rpc_pid, "ShareLock").await;
    let rpc_holds_exclusive: bool = observer
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND mode='ExclusiveLock' AND granted)",
            &[&rpc_pid],
        )
        .await
        .unwrap()
        .get(0);
    assert!(
        !rpc_holds_exclusive,
        "RPC acquired the call ledger while still queued for the authorization fence"
    );

    // The HTTP preparation already owns shared authorization. PostgreSQL must
    // allow its reentrant shared acquire, despite the queued exclusive writer,
    // so it can acquire the still-free ledger and commit without a lock cycle.
    tokio::time::timeout(Duration::from_secs(1), db::call_lock(&prepared))
        .await
        .expect("Prepared HTTP transaction deadlocked behind the queued auth writer")
        .unwrap();
    prepared.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), acquired)
        .await
        .expect("Exclusive auth writer did not proceed after HTTP preparation committed")
        .unwrap();
    assert!(
        !rpc.is_finished(),
        "RPC bypassed the now-exclusive authorization writer"
    );

    let cancel_connection = f.app.db.connect_scoped(timeout).await.unwrap();
    let cancel_pid: i32 = cancel_connection
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let cancel = tokio::spawn(ledger_user(cancel_connection, occupied.clone(), "cancel"));
    queued_advisory_lock(&observer, cancel_pid, "ShareLock").await;
    assert!(!cancel.is_finished());
    release_writer.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        writer.await.unwrap();
        rpc.await.unwrap();
        cancel.await.unwrap();
    })
    .await
    .expect("Writer/RPC/cancel transactions did not finish after releasing the auth fence");
    assert_eq!(occupied.load(std::sync::atomic::Ordering::SeqCst), 0);
    let leaked: bool = observer
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid IN ($1,$2,$3) AND locktype='advisory')",
            &[&writer_pid, &rpc_pid, &cancel_pid],
        )
        .await
        .unwrap()
        .get(0);
    assert!(!leaked, "Completed transactions retained advisory locks");
}

async fn grant_expiry_across_ledger_barrier(surviving_alternative: bool) {
    let Some(f) = crate::backups::tests::Fixture::new().await else {
        return;
    };
    let mut setup_connection = f
        .app
        .db
        .connect_scoped(Duration::from_secs(15))
        .await
        .unwrap();
    let setup = setup_connection.transaction().await.unwrap();
    db::auth_lock(&setup, true).await.unwrap();
    let setup_ctx = f
        .app
        .auth
        .authenticate(
            &setup,
            &f.token,
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    let native:Uuid=setup.query_one("SELECT g.id FROM grants g JOIN accounts a ON a.id=g.subject_id WHERE g.subject_id=$1 AND g.issuer_id=g.subject_id AND g.issuer_credential_id IS NULL AND g.role_id='Owner' AND g.scope->>'kind'='plugin' AND g.created_at=a.created_at",&[&setup_ctx.subject]).await.unwrap().get(0);
    let expiring = if surviving_alternative {
        // Publish a constrained group and grant through the existing identity
        // implementation. Its issuing authority is the unchanged native Owner
        // grant; no fabricated Owner anchor or self-escalation is introduced.
        let empty = std::collections::HashMap::new();
        let headers = HeaderMap::new();
        let group_body = json!({"name":"short-lived HTTP alternative","rules":[{"action":"plugin.http.invoke","resource":{"kind":"plugin","match":"ids","ids":["org.xelab.http-echo"]}}]});
        validate("CreateGroup", &group_body).unwrap();
        crate::identity::authorize(&setup, &setup_ctx, "createGroup", &empty, &group_body)
            .await
            .unwrap();
        let group = crate::identity::execute(
            &setup,
            &f.app.auth,
            &setup_ctx,
            "createGroup",
            &empty,
            &group_body,
            &headers,
            "",
        )
        .await
        .unwrap();
        assert_eq!(group.1, 201);
        let params = std::collections::HashMap::from([(
            "account_id".to_owned(),
            setup_ctx.subject.to_string(),
        )]);
        let grant_body = json!({"source":"group","group_version":{"group_id":group.0["id"],"version":1},"scope":{"kind":"plugin","match":"ids","ids":["org.xelab.http-echo"]}});
        validate("GrantRequest", &grant_body).unwrap();
        crate::identity::authorize(&setup, &setup_ctx, "createGrant", &params, &grant_body)
            .await
            .unwrap();
        let grant = crate::identity::execute(
            &setup,
            &f.app.auth,
            &setup_ctx,
            "createGrant",
            &params,
            &grant_body,
            &headers,
            "",
        )
        .await
        .unwrap();
        assert_eq!(grant.1, 201);
        id(&grant.0, "id").unwrap()
    } else {
        native
    };
    let expiry = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
        chrono::Utc::now().timestamp_millis() + 5000,
    )
    .unwrap();
    setup
        .execute(
            "UPDATE grants SET expires_at=$2,revision=revision+1,updated_at=now() WHERE id=$1",
            &[&expiring, &expiry],
        )
        .await
        .unwrap();
    setup
        .execute(
            "UPDATE credentials SET conditions='{\"max_calls\":1}' WHERE id=$1",
            &[&setup_ctx.credential],
        )
        .await
        .unwrap();
    setup.commit().await.unwrap();

    let mut holder_connection = f
        .app
        .db
        .connect_scoped(Duration::from_secs(15))
        .await
        .unwrap();
    let held = holder_connection.transaction().await.unwrap();
    db::call_lock(&held).await.unwrap();
    let mut prepared_connection = f
        .app
        .db
        .connect_scoped(Duration::from_secs(15))
        .await
        .unwrap();
    let prepared = prepared_connection.transaction().await.unwrap();
    let pid: i32 = prepared
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let ctx = f
        .app
        .auth
        .authenticate(
            &prepared,
            &f.token,
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    let mut proof = crate::auth::CurrentAuthority::new(&prepared, &ctx)
        .await
        .unwrap();
    let resource = Resource::new("plugin", "org.xelab.http-echo", Some(ctx.subject));
    proof.bind_plugin(&prepared, &ctx, &resource).unwrap();
    proof
        .require(&prepared, &ctx, "plugin.http.invoke", &resource)
        .unwrap();
    let original = proof.paths(&prepared, &ctx, &resource).unwrap().to_vec();
    let expiring_paths = original
        .iter()
        .filter(|path| path.sources.contains(&expiring))
        .cloned()
        .collect::<Vec<_>>();
    assert!(!expiring_paths.is_empty());
    for path in &expiring_paths {
        assert_eq!(
            crate::policy::timestamp(&path.rule["conditions"]["expires_at"]).unwrap(),
            expiry,
            "Grant expiry was not carried into its prepared rules"
        );
    }
    let observer = f
        .app
        .db
        .connect_scoped(Duration::from_secs(15))
        .await
        .unwrap();
    {
        let waiting = db::call_lock(&prepared);
        tokio::pin!(waiting);
        tokio::select! {
            result=&mut waiting=>panic!("Prepared transaction bypassed held ledger: {result:?}"),
            _=queued_advisory_lock(&observer,pid,"ExclusiveLock")=>{}
        }
        let remaining = (expiry - chrono::Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO)
            + Duration::from_millis(30);
        tokio::select! {
            result=&mut waiting=>panic!("Prepared transaction bypassed held ledger before expiry: {result:?}"),
            _=tokio::time::sleep(remaining)=>{}
        }
        assert!(chrono::Utc::now() > expiry);
        held.commit().await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), &mut waiting)
            .await
            .expect("Expired proof never reached ledger admission")
            .unwrap();
    }
    // PostgreSQL now() is frozen at transaction start. These checks must use
    // the carried wall-clock condition instead of trusting that old snapshot.
    let expired = crate::identity::call_sources_from_paths(
        &prepared,
        &ctx,
        "plugin.http.invoke",
        &resource,
        expiring_paths,
    )
    .await
    .unwrap_err();
    assert_eq!(expired.code, "PERMISSION_DENIED");
    if surviving_alternative {
        assert!(original
            .iter()
            .any(|path| path.sources.contains(&native) && !path.sources.contains(&expiring)));
        proof
            .require(&prepared, &ctx, "plugin.http.invoke", &resource)
            .unwrap();
        let funding = crate::identity::call_sources_from_paths(
            &prepared,
            &ctx,
            "plugin.http.invoke",
            &resource,
            original,
        )
        .await
        .unwrap();
        assert!(funding.contains(&ctx.credential));
        let call = Uuid::new_v4();
        let contract = json!({"name":"org.xelab.http-echo.http","version":"1.0.0","schema_digest":digest(b"expiry funding")});
        prepared.execute("INSERT INTO calls(id,owner_id,credential_id,contract,input_digest,state,deadline_at,created_at) VALUES($1,$2,$3,$4,$5,'running',now()+interval '30 seconds',now())",&[&call,&ctx.subject,&ctx.credential,&contract,&digest(b"expiry funding input")]).await.unwrap();
        for credential in &funding {
            prepared
                .execute(
                    "INSERT INTO call_credential_charges(call_id,credential_id) VALUES($1,$2)",
                    &[&call, credential],
                )
                .await
                .unwrap();
        }
        assert_eq!(
            prepared
                .query_one(
                    "SELECT count(*) FROM call_credential_charges WHERE call_id=$1",
                    &[&call]
                )
                .await
                .unwrap()
                .get::<_, i64>(0),
            funding.len() as i64
        );
    } else {
        assert_eq!(
            proof
                .require(&prepared, &ctx, "plugin.http.invoke", &resource)
                .unwrap_err()
                .code,
            "PERMISSION_DENIED"
        );
        assert_eq!(
            crate::identity::call_sources_from_paths(
                &prepared,
                &ctx,
                "plugin.http.invoke",
                &resource,
                original
            )
            .await
            .unwrap_err()
            .code,
            "PERMISSION_DENIED"
        );
        assert_eq!(
            prepared
                .query_one("SELECT count(*) FROM calls", &[])
                .await
                .unwrap()
                .get::<_, i64>(0),
            0
        );
        assert_eq!(
            prepared
                .query_one("SELECT count(*) FROM call_credential_charges", &[])
                .await
                .unwrap()
                .get::<_, i64>(0),
            0
        );
        let quota = prepared
            .query_one(
                "SELECT concurrent_calls_used,daily_calls_used FROM quotas WHERE account_id=$1",
                &[&ctx.subject],
            )
            .await
            .unwrap();
        assert_eq!(quota.get::<_, i64>(0), 0);
        assert_eq!(quota.get::<_, i64>(1), 0);
    }
    drop(proof);
    prepared.rollback().await.unwrap();
}

#[tokio::test]
async fn prepared_authority_and_funding_expire_while_waiting_for_the_actual_call_ledger() {
    grant_expiry_across_ledger_barrier(false).await;
}

#[tokio::test]
async fn an_original_native_alternative_survives_an_expired_prepared_group_grant() {
    grant_expiry_across_ledger_barrier(true).await;
}

async fn expiry_fixture_identity_operation(
    tx: &tokio_postgres::Transaction<'_>,
    app: &App,
    ctx: &Context,
    operation: &str,
    schema: &str,
    params: &std::collections::HashMap<String, String>,
    body: Value,
) -> Value {
    validate(schema, &body).unwrap();
    crate::identity::authorize(tx, ctx, operation, params, &body)
        .await
        .unwrap_or_else(|error| panic!("{operation} fixture authorization failed: {error:?}"));
    let (response, status) = crate::identity::execute(
        tx,
        &app.auth,
        ctx,
        operation,
        params,
        &body,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap_or_else(|error| panic!("{operation} fixture execution failed: {error:?}"));
    assert_eq!(status, 201);
    response
}

async fn issuer_account_expiry_across_ledger_barrier(surviving_alternative: bool) {
    let Some(f) = crate::backups::tests::Fixture::new().await else {
        return;
    };
    let mut setup_connection = f
        .app
        .db
        .connect_scoped(Duration::from_secs(20))
        .await
        .unwrap();
    let setup = setup_connection.transaction().await.unwrap();
    db::auth_lock(&setup, true).await.unwrap();
    let owner = f
        .app
        .auth
        .authenticate(
            &setup,
            &f.token,
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    let owner_params =
        std::collections::HashMap::from([("account_id".to_owned(), owner.subject.to_string())]);
    // Every source below is issued by the existing identity implementation.
    // The child inherits the genuine native Owner authority; no additional
    // Owner anchor, role grant, or synthetic credential is inserted.
    let child = expiry_fixture_identity_operation(
        &setup, &f.app, &owner, "createChildAccount", "CreateChild", &owner_params,
        json!({"display_name":"expiring HTTP source issuer","kind":"service","inheritance":{"mode":"all_snapshot"}}),
    ).await;
    let issuer = id(&child["account"], "id").unwrap();
    let child_params =
        std::collections::HashMap::from([("account_id".to_owned(), issuer.to_string())]);
    let key_expiry = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
        chrono::Utc::now().timestamp_millis() + 86_400_000,
    )
    .unwrap();
    let key = expiry_fixture_identity_operation(
        &setup, &f.app, &owner, "createApiKey", "CreateApiKey", &child_params,
        json!({"name":"real HTTP grant issuer","inheritance":{"mode":"all_snapshot"},"conditions":{"expires_at":key_expiry}}),
    ).await;
    let issuer_key = id(&key["credential"], "id").unwrap();
    let issuer_context = f
        .app
        .auth
        .authenticate(
            &setup,
            key["secret"].as_str().unwrap(),
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    assert_eq!(issuer_context.subject, issuer);
    assert_eq!(issuer_context.credential, issuer_key);
    let group = expiry_fixture_identity_operation(
        &setup, &f.app, &issuer_context, "createGroup", "CreateGroup", &std::collections::HashMap::new(),
        // A key cannot publish a template beyond its own non-action bounds.
        // This is deliberately much later than the issuing account's expiry.
        json!({"name":"derived HTTP issuer authority","rules":[{"action":"plugin.http.invoke","resource":{"kind":"plugin","match":"ids","ids":["org.xelab.backup-fixture"]},"conditions":{"expires_at":key_expiry}}]}),
    ).await;
    let grant = expiry_fixture_identity_operation(
        &setup, &f.app, &issuer_context, "createGrant", "GrantRequest", &owner_params,
        json!({"source":"group","group_version":{"group_id":group["id"],"version":1},"scope":{"kind":"plugin","match":"ids","ids":["org.xelab.backup-fixture"]}}),
    ).await;
    let derived = id(&grant, "id").unwrap();
    let source = setup
        .query_one(
            "SELECT issuer_id,issuer_credential_id FROM grants WHERE id=$1",
            &[&derived],
        )
        .await
        .unwrap();
    assert_eq!(source.get::<_, Uuid>(0), issuer);
    assert_eq!(source.get::<_, Option<Uuid>>(1), Some(issuer_key));
    // Set the real issuing account's lifetime only after legitimate issuance;
    // the grant and API key themselves deliberately have a longer lifetime.
    let expiry = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
        chrono::Utc::now().timestamp_millis() + 5000,
    )
    .unwrap();
    setup
        .execute(
            "UPDATE accounts SET expires_at=$2,revision=revision+1,updated_at=now() WHERE id=$1",
            &[&issuer, &expiry],
        )
        .await
        .unwrap();
    setup.commit().await.unwrap();

    let mut holder_connection = f
        .app
        .db
        .connect_scoped(Duration::from_secs(15))
        .await
        .unwrap();
    let held = holder_connection.transaction().await.unwrap();
    db::call_lock(&held).await.unwrap();
    let mut prepared_connection = f
        .app
        .db
        .connect_scoped(Duration::from_secs(15))
        .await
        .unwrap();
    let prepared = prepared_connection.transaction().await.unwrap();
    let pid: i32 = prepared
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let ctx = f
        .app
        .auth
        .authenticate(
            &prepared,
            &f.token,
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    let resource = Resource::new("plugin", "org.xelab.backup-fixture", Some(ctx.subject));
    let mut authority = crate::auth::CurrentAuthority::new(&prepared, &ctx)
        .await
        .unwrap();
    authority.bind_plugin(&prepared, &ctx, &resource).unwrap();
    authority
        .require(&prepared, &ctx, "plugin.http.invoke", &resource)
        .unwrap();
    let original = authority
        .paths(&prepared, &ctx, &resource)
        .unwrap()
        .to_vec();
    let issued_paths: Vec<_> = original
        .iter()
        .filter(|path| path.sources.contains(&derived))
        .cloned()
        .collect();
    assert!(
        !issued_paths.is_empty(),
        "The legitimate child-issued grant was absent from preparation"
    );
    // Prove that this is a usable, correctly charged alternative before expiry.
    let before = crate::identity::call_sources_from_paths(
        &prepared,
        &ctx,
        "plugin.http.invoke",
        &resource,
        issued_paths.clone(),
    )
    .await
    .unwrap();
    assert!(before.contains(&issuer_key));
    assert!(before.contains(&ctx.credential));
    let observer = f
        .app
        .db
        .connect_scoped(Duration::from_secs(15))
        .await
        .unwrap();
    {
        let waiting = db::call_lock(&prepared);
        tokio::pin!(waiting);
        tokio::select! {
            result=&mut waiting=>panic!("Prepared issuer proof bypassed held ledger: {result:?}"),
            _=queued_advisory_lock(&observer,pid,"ExclusiveLock")=>{}
        }
        let remaining = (expiry - chrono::Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO)
            + Duration::from_millis(30);
        tokio::select! {
            result=&mut waiting=>panic!("Prepared issuer proof bypassed held ledger before expiry: {result:?}"),
            _=tokio::time::sleep(remaining)=>{}
        }
        assert!(chrono::Utc::now() > expiry);
        held.commit().await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), &mut waiting)
            .await
            .expect("Issuer-expiry proof did not reach the call ledger")
            .unwrap();
    }
    assert_eq!(
        crate::auth::validate_sources(&prepared, Some(issuer_key), Some(issuer), ctx.peer)
            .await
            .unwrap_err()
            .code,
        "INVALID_CREDENTIAL"
    );
    // The mandatory caller is still live. Only the expired alternative is
    // withdrawn; an INVALID_CREDENTIAL from an optional issuer is not global.
    assert!(crate::auth::validate_sources(
        &prepared,
        Some(ctx.credential),
        Some(ctx.subject),
        ctx.peer
    )
    .await
    .is_ok());
    assert_eq!(
        crate::identity::call_sources_from_paths(
            &prepared,
            &ctx,
            "plugin.http.invoke",
            &resource,
            issued_paths
        )
        .await
        .unwrap_err()
        .code,
        "PERMISSION_DENIED"
    );
    if surviving_alternative {
        let funding = crate::identity::call_sources_from_paths(
            &prepared,
            &ctx,
            "plugin.http.invoke",
            &resource,
            original,
        )
        .await
        .unwrap();
        assert!(funding.contains(&ctx.credential));
        assert!(!funding.contains(&issuer_key));
        let deployment = prepared.query_one("SELECT d.id,d.installation_id FROM deployments d JOIN installations n ON n.id=d.installation_id WHERE n.plugin_id='org.xelab.backup-fixture'", &[]).await.unwrap();
        let deployment_id: Uuid = deployment.get(0);
        let installation: Uuid = deployment.get(1);
        let call = Uuid::new_v4();
        let hash = digest(b"issuer expiry metadata unit fixture");
        let contract =
            json!({"name":"org.xelab.backup-fixture.http","version":"1.0.0","schema_digest":hash});
        let binding = json!({"installation_id":installation,"deployment_id":deployment_id,"generation":1,"http_contract_digest":hash,"operation_id":"fixture_echo","method":"POST","path":"/plugins/org.xelab.backup-fixture/echo","required_action":"plugin.http.invoke"});
        prepared.execute("INSERT INTO calls(id,owner_id,credential_id,contract,input_digest,state,deadline_at,created_at,http_binding) VALUES($1,$2,$3,$4,$5,'running',now()+interval '30 seconds',now(),$6)", &[&call,&ctx.subject,&ctx.credential,&contract,&hash,&binding]).await.unwrap();
        for credential in &funding {
            prepared
                .execute(
                    "INSERT INTO call_credential_charges(call_id,credential_id) VALUES($1,$2)",
                    &[&call, credential],
                )
                .await
                .unwrap();
        }
        // Exercise the actual pinned HTTP context freezer without needing Blob
        // declarations or pretending this ledger unit fixture ran an OCI call.
        crate::host_blobs::grant_http_call(
            &prepared,
            &f.app,
            &ctx,
            call,
            deployment_id,
            &json!({}),
            &json!({"input_schema":{}}),
            &json!({}),
            false,
            &authority,
        )
        .await
        .unwrap();
        let frozen: Value = prepared
            .query_one(
                "SELECT source_paths FROM host_rpc_contexts WHERE call_id=$1",
                &[&call],
            )
            .await
            .unwrap()
            .get(0);
        let frozen: Vec<crate::policy::Path> = serde_json::from_value(frozen).unwrap();
        assert!(!frozen.is_empty());
        assert!(
            frozen.iter().all(|path| !path.sources.contains(&derived)),
            "Frozen HTTP context retained an expired, uncharged issuer"
        );
        assert_eq!(prepared.query_one("SELECT count(*) FROM call_credential_charges WHERE call_id=$1 AND credential_id=$2", &[&call,&issuer_key]).await.unwrap().get::<_,i64>(0), 0);
        assert_eq!(
            prepared
                .query_one(
                    "SELECT count(*) FROM call_credential_charges WHERE call_id=$1",
                    &[&call]
                )
                .await
                .unwrap()
                .get::<_, i64>(0),
            funding.len() as i64
        );
    } else {
        assert_eq!(
            prepared
                .query_one("SELECT count(*) FROM calls", &[])
                .await
                .unwrap()
                .get::<_, i64>(0),
            0
        );
        assert_eq!(
            prepared
                .query_one("SELECT count(*) FROM call_credential_charges", &[])
                .await
                .unwrap()
                .get::<_, i64>(0),
            0
        );
        let quota = prepared
            .query_one(
                "SELECT concurrent_calls_used,daily_calls_used FROM quotas WHERE account_id=$1",
                &[&ctx.subject],
            )
            .await
            .unwrap();
        assert_eq!(quota.get::<_, i64>(0), 0);
        assert_eq!(quota.get::<_, i64>(1), 0);
    }
    drop(authority);
    prepared.rollback().await.unwrap();
}

#[tokio::test]
async fn an_expired_prepared_issuer_account_does_not_poison_a_live_owner_alternative() {
    issuer_account_expiry_across_ledger_barrier(true).await;
}

#[tokio::test]
async fn an_expired_prepared_issuer_account_alone_cannot_admit_or_charge_a_call() {
    issuer_account_expiry_across_ledger_barrier(false).await;
}

#[tokio::test]
async fn cancellation_and_deadline_frames_preserve_consumer_sequence_and_canonical_reason() {
    let (output, _receiver) = output();
    let call = Uuid::new_v4();
    output.bind(call, chrono::Utc::now() + chrono::Duration::seconds(30));
    for (code, reason, expected) in [
        ("CANCELED", "requested", 3u64),
        ("DEADLINE_EXCEEDED", "deadline", 4u64),
    ] {
        let (bytes, terminal, sequence) = error_frame(
            &output.identity,
            Error::new(code, "Call stopped", StatusCode::CONFLICT),
            expected,
        )
        .unwrap();
        assert!(terminal);
        assert_eq!(sequence, expected);
        let frame = std::str::from_utf8(&bytes).unwrap();
        assert!(frame.starts_with("event: canceled\n"));
        assert!(frame.contains(&format!("id: {call}:{expected}\n")));
        let data: Value = serde_json::from_str(
            frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .unwrap(),
        )
        .unwrap();
        validate("StreamEvent", &data).unwrap();
        assert_eq!(data["call_id"], call.to_string());
        assert_eq!(data["sequence"], expected);
        assert_eq!(data["terminal"], true);
        assert_eq!(data["payload"], json!({"reason":reason}));
    }
}
