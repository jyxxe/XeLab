use super::*;

fn spec() -> Value {
    json!({
        "name":"A public UI",
        "plugin_id":"org.xelab.webui",
        "allowed_origins":["https://ui.example.org","https://ui.example.org:9443"],
        "redirect_uris":["https://ui.example.org/callback?app=desktop","https://ui.example.org:9443/auth/complete"],
        "maximum_rules":[{"action":"platform.read","resource":{"kind":"platform","match":"all"}}]
    })
}

fn client(spec: Value) -> Client {
    Client {
        id: Uuid::new_v4(),
        client_id: "xelab-unit-public-client".into(),
        created_by: Some(Uuid::new_v4()),
        spec,
        revision: 1,
        status: "active".into(),
    }
}

#[test]
fn callback_registration_requires_exact_https_origins_and_unambiguous_oauth_query() {
    validate_spec(&spec()).unwrap();
    for callback in [
        "http://ui.example.org/callback",
        "http://127.0.0.1:8888/callback",
        "https://attacker.invalid/callback",
        "https://user:password@ui.example.org/callback",
        "https://ui.example.org/callback#fragment",
        "https://*.example.org/callback",
        "https://ui.example.org/callback*",
        "https://ui.example.org\\@attacker.invalid/callback",
        "https://UI.EXAMPLE.ORG/callback",
        "https://ui.example.org:443/callback",
        "https://ui.example.org/a/../callback",
        "https://ui.example.org/callback\r\nLocation: https://attacker.invalid",
        "https://ui.example.org/callback?code=other",
        "https://ui.example.org/callback?%63ode=other",
        "https://ui.example.org/callback?state=other",
        "https://ui.example.org/callback?error=other",
        "https://ui.example.org/callback?error_description=other",
    ] {
        let mut invalid = spec();
        invalid["redirect_uris"] = json!([callback]);
        assert!(validate_spec(&invalid).is_err(), "{callback}");
    }
    for origin in [
        "http://ui.example.org",
        "https://ui.example.org/",
        "https://ui.example.org/path",
        "https://ui.example.org?query=value",
        "https://ui.example.org#fragment",
        "https://*.example.org",
        "https://user@ui.example.org",
        "https://UI.EXAMPLE.ORG",
        "https://ui.example.org:443",
        "null",
    ] {
        let mut invalid = spec();
        invalid["allowed_origins"] = json!([origin]);
        assert!(validate_spec(&invalid).is_err(), "{origin}");
    }
    for field in ["redirect_uris", "allowed_origins"] {
        let mut invalid = spec();
        invalid[field] = json!([invalid[field][0], invalid[field][0]]);
        assert!(validate_spec(&invalid).is_err());
    }
    for field in ["id", "client_id", "created_by", "status", "revision"] {
        let mut invalid = spec();
        invalid[field] = json!("caller-supplied protected value");
        assert!(validate_spec(&invalid).is_err(), "{field}");
    }
}

#[test]
fn scope_handles_are_registered_exactly_and_never_become_authority() {
    let mut specification = spec();
    let second = json!({"action":"client.read","resource":{"kind":"client","match":"owned_by","owner_id":Uuid::new_v4()}});
    specification["maximum_rules"]
        .as_array_mut()
        .unwrap()
        .push(second.clone());
    let registered = client(specification);
    validate_spec(&registered.spec).unwrap();
    let handle = format!(
        "rule:{}",
        crate::skills::definition_digest(&second).unwrap()
    );
    let (normalized, selected) =
        requested_rules(&registered, &format!(" {handle}  {handle} ")).unwrap();
    assert_eq!(normalized, handle);
    assert_eq!(selected, json!([second]));
    let full = format!("client:{}", registered.client_id);
    let (normalized, selected) =
        requested_rules(&registered, &format!("{handle} {full} {full}")).unwrap();
    let mut expected = [full.clone(), handle.clone()];
    expected.sort();
    assert_eq!(normalized, expected.join(" "));
    assert_eq!(selected.as_array().unwrap().len(), 2);
    assert!(selected.as_array().unwrap().contains(&second));
    assert!(selected
        .as_array()
        .unwrap()
        .contains(&registered.spec["maximum_rules"][0]));
    for scope in [
        "".to_owned(),
        "   ".to_owned(),
        "client:another-public-client".to_owned(),
        format!("{full} client:another-public-client"),
        "rule:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        "platform.read".to_owned(),
        format!("{full}\t{handle}"),
        format!("{full}\n{handle}"),
        format!("{full}\u{a0}{handle}"),
        "x".repeat(513),
    ] {
        assert!(requested_rules(&registered, &scope).is_err(), "{scope:?}");
    }
    let mut disabled = registered;
    disabled.status = "disabled".into();
    assert!(requested_rules(&disabled, &full).is_err());
}

fn revision(value: i64) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("if-match", format!("\"rev-{value}\"").parse().unwrap());
    headers
}

// These metadata rows exercise client invalidation, not an OAuth exchange.
// Authorization-code/PKCE/refresh execution is covered by the OAuth flow suite.
async fn bound_state(
    c: &(impl GenericClient + Sync),
    auth: &Auth,
    ctx: &Context,
    public: &str,
    client_revision: i64,
) -> (Uuid, [Uuid; 2], Uuid, Uuid) {
    let family = Uuid::new_v4();
    c.execute("INSERT INTO oauth_families(id,subject_id,session_id,client_id,client_revision,origin,scope,maximum_rules,fixed_paths,expires_at,status,created_at,updated_at) VALUES($1,$2,$3,$4,$5,'https://ui.example.org',$6,'[]','[]',now()+interval '1 hour','active',now(),now())", &[&family,&ctx.subject,&ctx.credential,&public,&client_revision,&format!("client:{public}")]).await.unwrap();
    let credentials = [Uuid::new_v4(), Uuid::new_v4()];
    for (credential, kind) in credentials.iter().zip(["oauth_access", "oauth_refresh"]) {
        c.execute("INSERT INTO credentials(id,owner_id,kind,hmac_key_id,verification_digest,issuer_credential_id,client_id,family_id,status,inheritance_mode,maximum_rules,conditions,expires_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'active','selected','[]','{}',now()+interval '1 hour',now(),now())", &[credential,&ctx.subject,&kind,&auth.key_id.as_str(),&auth.token_digest(&credential.to_string()),&ctx.credential,&public,&family]).await.unwrap();
    }
    let request = Uuid::new_v4();
    c.execute("INSERT INTO authorization_requests(id,session_id,client_id,redirect_uri,state,code_challenge,fixed_rules,csrf_digest,expires_at,status,scope,origin,client_revision) VALUES($1,$2,$3,'https://ui.example.org/callback','opaque-state',$4,'[]',$5,now()+interval '60 seconds','pending',$6,'https://ui.example.org',$7)", &[&request,&ctx.credential,&public,&"A".repeat(43),&auth.token_digest(&request.to_string()),&format!("client:{public}"),&client_revision]).await.unwrap();
    let intent = Uuid::new_v4();
    c.execute("INSERT INTO oauth_login_intents(id,hmac_key_id,verification_digest,client_id,client_revision,request_spec,peer,expires_at,created_at) VALUES($1,$2,$3,$4,$5,'{}','127.0.0.1',now()+interval '60 seconds',now())", &[&intent,&auth.key_id.as_str(),&auth.token_digest(&intent.to_string()),&public,&client_revision]).await.unwrap();
    (family, credentials, request, intent)
}

#[tokio::test]
async fn actual_postgres_clients_keep_public_identity_filter_visibility_and_revoke_bound_state() {
    let Some(fixture) = crate::backups::tests::Fixture::new().await else {
        return;
    };
    let mut c = fixture.app.db.connect().await.unwrap();
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, true).await.unwrap();
    let ctx = fixture
        .app
        .auth
        .authenticate(
            &tx,
            &fixture.token,
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    let auth = &fixture.app.auth;
    let empty = HashMap::new();
    authorize(&tx, &ctx, "createClient", &empty, &spec())
        .await
        .unwrap();
    let (created, code) = execute(
        &tx,
        auth,
        &ctx,
        "createClient",
        &empty,
        &spec(),
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap();
    assert_eq!(code, 201);
    validate("Client", &created).unwrap();
    assert!(
        created.get("created_by").is_none(),
        "internal ownership must not add a public field"
    );
    let id = crate::contracts::id(&created, "id").unwrap();
    let public = created["client_id"].as_str().unwrap();
    let params = HashMap::from([("client_id".to_owned(), id.to_string())]);
    let fixed = by_public_id(&tx, public).await.unwrap();
    assert_eq!(fixed.id, id);
    assert_eq!(fixed.created_by, Some(ctx.subject));
    let (other, _) = execute(
        &tx,
        auth,
        &ctx,
        "createClient",
        &empty,
        &spec(),
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap();
    let other_public = other["client_id"].as_str().unwrap();
    let other_id = crate::contracts::id(&other, "id").unwrap();
    let (page, _) = execute(
        &tx,
        auth,
        &ctx,
        "listClients",
        &empty,
        &Value::Null,
        &HeaderMap::new(),
        "limit=1",
    )
    .await
    .unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let cursor = page["next_cursor"].as_str().unwrap();
    let next = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("limit", "1")
        .append_pair("cursor", cursor)
        .finish();
    let (second, _) = execute(
        &tx,
        auth,
        &ctx,
        "listClients",
        &empty,
        &Value::Null,
        &HeaderMap::new(),
        &next,
    )
    .await
    .unwrap();
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_ne!(page["items"][0]["id"], second["items"][0]["id"]);
    assert!(second["next_cursor"].is_null());
    let account = HashMap::from([("account_id".to_owned(), ctx.subject.to_string())]);
    let (key, _) = crate::identity::execute(&tx, auth, &ctx, "createApiKey", &account, &json!({"name":"one Client reader","inheritance":{"mode":"selected","rules":[{"action":"client.read","resource":{"kind":"client","match":"ids","ids":[id]}}]},"conditions":{"expires_at":Utc::now()+chrono::Duration::hours(1)}}), &HeaderMap::new(), "").await.unwrap();
    let limited = auth
        .authenticate(
            &tx,
            key["secret"].as_str().unwrap(),
            ctx.peer,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    assert_eq!(
        authorize(
            &tx,
            &limited,
            "getClient",
            &HashMap::from([("client_id".to_owned(), other_id.to_string())]),
            &Value::Null
        )
        .await
        .unwrap_err()
        .code,
        "RESOURCE_NOT_FOUND"
    );
    assert_eq!(
        authorize(&tx, &limited, "updateClient", &params, &spec())
            .await
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
    assert!(
        execute(
            &tx,
            auth,
            &limited,
            "listClients",
            &empty,
            &Value::Null,
            &HeaderMap::new(),
            &next
        )
        .await
        .is_err(),
        "cursor cannot cross credentials"
    );
    let (visible, _) = execute(
        &tx,
        auth,
        &limited,
        "listClients",
        &empty,
        &Value::Null,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap();
    assert_eq!(visible["items"].as_array().unwrap().len(), 1);
    assert_eq!(visible["items"][0]["id"], created["id"]);
    for query in [
        "limit=0",
        "limit=201",
        "limit=1&limit=2",
        "origin=https%3A%2F%2Fevil.invalid",
        "cursor=invalid",
    ] {
        assert!(
            execute(
                &tx,
                auth,
                &ctx,
                "listClients",
                &empty,
                &Value::Null,
                &HeaderMap::new(),
                query
            )
            .await
            .is_err(),
            "{query}"
        );
    }
    let (registrar, _) = crate::identity::execute(
        &tx,
        auth,
        &ctx,
        "createApiKey",
        &account,
        &json!({"name":"Client management without business authority","inheritance":{"mode":"selected","rules":[{"action":"client.create","resource":{"kind":"client","match":"owned_by","owner_id":ctx.subject}},{"action":"client.update","resource":{"kind":"client","match":"ids","ids":[id]}}]},"conditions":{"expires_at":Utc::now()+chrono::Duration::hours(1)}}),
        &HeaderMap::new(),
        "",
    ).await.unwrap();
    let registrar = auth
        .authenticate(
            &tx,
            registrar["secret"].as_str().unwrap(),
            ctx.peer,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    authorize(&tx, &registrar, "createClient", &empty, &spec())
        .await
        .unwrap();
    assert_eq!(
        execute(
            &tx,
            auth,
            &registrar,
            "createClient",
            &empty,
            &spec(),
            &HeaderMap::new(),
            ""
        )
        .await
        .unwrap_err()
        .code,
        "PERMISSION_DENIED",
        "client.create must not delegate a business rule outside the actual credential ceiling"
    );
    authorize(&tx, &registrar, "updateClient", &params, &spec())
        .await
        .unwrap();
    assert_eq!(
        execute(
            &tx,
            auth,
            &registrar,
            "updateClient",
            &params,
            &spec(),
            &revision(1),
            ""
        )
        .await
        .unwrap_err()
        .code,
        "PERMISSION_DENIED"
    );
    let mut unknown = spec();
    unknown["maximum_rules"][0]["action"] = json!("xelab.unregistered-client-test-action");
    assert_eq!(
        execute(
            &tx,
            auth,
            &ctx,
            "createClient",
            &empty,
            &unknown,
            &HeaderMap::new(),
            ""
        )
        .await
        .unwrap_err()
        .code,
        "INVALID_ARGUMENT"
    );
    assert_eq!(
        tx.query_one("SELECT count(*) FROM clients", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        2
    );
    assert_eq!(by_id(&tx, id).await.unwrap().revision, 1);
    let first_state = bound_state(&tx, auth, &ctx, public, 1).await;
    let unaffected = bound_state(&tx, auth, &ctx, other_public, 1).await;
    let mut updated_spec = spec();
    updated_spec["name"] = json!("Updated public UI");
    let (updated, _) = execute(
        &tx,
        auth,
        &ctx,
        "updateClient",
        &params,
        &updated_spec,
        &revision(1),
        "",
    )
    .await
    .unwrap();
    assert_eq!(updated["client_id"], created["client_id"]);
    assert_eq!(updated["id"], created["id"]);
    assert_eq!(updated["revision"], 2);
    assert_eq!(by_id(&tx, id).await.unwrap().created_by, Some(ctx.subject));
    assert_eq!(
        execute(
            &tx,
            auth,
            &ctx,
            "updateClient",
            &params,
            &updated_spec,
            &revision(1),
            ""
        )
        .await
        .unwrap_err()
        .code,
        "ETAG_MISMATCH"
    );
    for credential in first_state.1 {
        assert_eq!(
            tx.query_one("SELECT status FROM credentials WHERE id=$1", &[&credential])
                .await
                .unwrap()
                .get::<_, String>(0),
            "revoked"
        );
    }
    assert_eq!(
        tx.query_one(
            "SELECT status FROM oauth_families WHERE id=$1",
            &[&first_state.0]
        )
        .await
        .unwrap()
        .get::<_, String>(0),
        "revoked"
    );
    assert_eq!(
        tx.query_one(
            "SELECT status FROM authorization_requests WHERE id=$1",
            &[&first_state.2]
        )
        .await
        .unwrap()
        .get::<_, String>(0),
        "denied"
    );
    assert!(!tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM oauth_login_intents WHERE id=$1)",
            &[&first_state.3]
        )
        .await
        .unwrap()
        .get::<_, bool>(0));
    assert_eq!(
        tx.query_one(
            "SELECT status FROM oauth_families WHERE id=$1",
            &[&unaffected.0]
        )
        .await
        .unwrap()
        .get::<_, String>(0),
        "active"
    );
    let active = bound_state(&tx, auth, &ctx, public, 2).await;
    let (_, code) = execute(
        &tx,
        auth,
        &ctx,
        "disableClient",
        &params,
        &Value::Null,
        &revision(2),
        "",
    )
    .await
    .unwrap();
    assert_eq!(code, 204);
    assert_eq!(by_public_id(&tx, public).await.unwrap().status, "disabled");
    assert_eq!(
        execute(
            &tx,
            auth,
            &ctx,
            "updateClient",
            &params,
            &updated_spec,
            &revision(3),
            ""
        )
        .await
        .unwrap_err()
        .code,
        "STATE_CONFLICT"
    );
    assert_eq!(
        tx.query_one(
            "SELECT status FROM oauth_families WHERE id=$1",
            &[&active.0]
        )
        .await
        .unwrap()
        .get::<_, String>(0),
        "revoked"
    );
    assert_eq!(
        tx.query_one(
            "SELECT status FROM credentials WHERE id=$1",
            &[&ctx.credential]
        )
        .await
        .unwrap()
        .get::<_, String>(0),
        "active",
        "Client lifecycle must preserve the independent trusted login session"
    );
    assert_eq!(
        tx.query_one(
            "SELECT status FROM credentials WHERE id=$1",
            &[&unaffected.1[0]]
        )
        .await
        .unwrap()
        .get::<_, String>(0),
        "active",
        "another Client's token family must survive"
    );
    tx.rollback().await.unwrap();
}
