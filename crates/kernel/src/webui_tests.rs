//! Metadata, routing and authorization components; these do not claim a
//! container launch or browser/OCI acceptance.
use super::*;
use axum::http::HeaderValue;

fn manifest() -> Value {
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../docs/preset-plugins/webui/manifest.json"
    ))
    .unwrap();
    value["concurrency"]["max_queue_requests"] = json!(0);
    value
}

#[test]
fn webui_metadata_rejects_namespace_origin_and_unsigned_route_substitution() {
    let signed = manifest();
    crate::artifact::validate_profile(&signed).unwrap();
    assert_eq!(entry_path(&signed).unwrap(), "/ui/org.xelab.webui/");
    for (field, value) in [
        ("base_path", json!("/oauth/")),
        ("entry_path", json!("/../auth/ui/login")),
        ("entry_path", json!("/%2e%2e/")),
        ("entry_path", json!("//other.example/")),
        ("entry_path", json!("/callback?next=bad")),
        ("origin", json!("https://evil.example/path")),
        ("origin", json!("https://user@evil.example")),
        ("isolation", json!("sandbox")),
    ] {
        let mut invalid = signed.clone();
        invalid["webui"][field] = value;
        assert!(
            crate::artifact::validate_profile(&invalid).is_err(),
            "{field}"
        );
    }
    let mut missing = signed.clone();
    missing.as_object_mut().unwrap().remove("webui");
    assert!(crate::artifact::validate_profile(&missing).is_err());
    let mut protected = signed.clone();
    protected["routes"][0]["authentication"] = json!("user");
    assert!(crate::artifact::validate_profile(&protected).is_err());
    let mut versioned = signed;
    versioned["data_schema_version"] = json!("1");
    assert!(
        crate::artifact::validate_profile(&versioned).is_err(),
        "UI support never opens the versioned artifact acceptance gate"
    );
}

#[tokio::test]
async fn actual_postgres_default_webui_pins_origin_callback_and_converges_on_disable() {
    let Some(mut f) = crate::backups::tests::Fixture::new().await else {
        return;
    };
    let mut config = f.app.config.value.clone();
    config["ui_gateway"] =
        json!({"base_domain":"plugins.example","public_port":443,"shell_origins":[]});
    // This component does not start a Runtime; only the trusted gateway's
    // configuration participates in the origin/selection checks below.
    std::sync::Arc::get_mut(&mut f.app).unwrap().config =
        crate::config::Config::from_value(config).unwrap();
    let mut c = f.app.db.connect().await.unwrap();
    let ctx = f
        .app
        .auth
        .authenticate(&c, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    assert!(redirect(&f.app).await.is_err());
    assert!(
        !c.query_one("SELECT EXISTS(SELECT 1 FROM default_ui)", &[])
            .await
            .unwrap()
            .get::<_, bool>(0),
        "anonymous lookup must not initialize state"
    );
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, true).await.unwrap();
    let (initial, _) = execute(
        &tx,
        &f.app,
        &ctx,
        "getDefaultUi",
        &Value::Null,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap();
    assert!(initial["deployment_id"].is_null());
    let row=tx.query_one("SELECT d.id,d.installation_id FROM backups b JOIN deployments d ON d.id=b.deployment_id WHERE b.id=$1",&[&f.backup]).await.unwrap();
    let deployment: Uuid = row.get("id");
    let installation: Uuid = row.get("installation_id");
    let signed = manifest();
    tx.execute("UPDATE installations SET plugin_id='org.xelab.webui',manifest=$2,manifest_digest=$3 WHERE id=$1",&[&installation,&signed,&crate::contracts::digest(&serde_json::to_vec(&signed).unwrap())]).await.unwrap();
    crate::http_contracts::store(&tx, installation, &signed, None)
        .await
        .unwrap();
    crate::plugin_http_gateway::approve(&tx, &ctx, deployment, &signed, &json!({}), ctx.subject)
        .await
        .unwrap();
    tx.execute("UPDATE deployments SET spec='{\"enabled\":true}',phase='ready',observed_generation=1 WHERE id=$1",&[&deployment]).await.unwrap();
    tx.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,last_heartbeat_at,created_at,updated_at) VALUES($1,$2,1,0,'ready','alive','{}',now(),now(),now())",&[&Uuid::new_v4(),&deployment]).await.unwrap();
    let origin = StaticOrigin::for_deployment(&f.app.config, deployment, None)
        .unwrap()
        .origin()
        .to_owned();
    let callback = format!("{origin}/ui/org.xelab.webui/callback?app=desktop");
    let client = json!({"name":"Fixed shell","plugin_id":"org.xelab.webui","redirect_uris":[callback],"allowed_origins":[origin],"maximum_rules":[{"action":"account.read","resource":{"kind":"account","match":"ids","ids":[ctx.subject]}}]});
    crate::oauth::clients::execute(
        &tx,
        &f.app.auth,
        &ctx,
        "createClient",
        &std::collections::HashMap::new(),
        &client,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("if-match", HeaderValue::from_static("\"rev-1\""));
    let (selected, _) = execute(
        &tx,
        &f.app,
        &ctx,
        "setDefaultUi",
        &json!({"deployment_id":deployment}),
        &headers,
        "",
    )
    .await
    .unwrap();
    assert_eq!(selected["revision"], 2);
    assert!(
        execute(
            &tx,
            &f.app,
            &ctx,
            "setDefaultUi",
            &json!({"deployment_id":null}),
            &headers,
            ""
        )
        .await
        .is_err(),
        "stale ETag must not clear the selection"
    );
    let fixed = check_deployment(&tx, &f.app, deployment).await.unwrap();
    assert_eq!(fixed.entry_url, format!("{origin}/ui/org.xelab.webui/"));
    let contribution = Uuid::new_v4();
    assert_eq!(
        frame_ancestors(&tx, &f.app, contribution).await.unwrap(),
        vec![origin.clone()]
    );
    assert!(
        frame_ancestors(&tx, &f.app, deployment)
            .await
            .unwrap()
            .is_empty(),
        "a shell is never an allowed parent of its own same-origin contribution"
    );
    let path = "/ui/org.xelab.webui/callback";
    validate_callback(
        &tx,
        &f.app,
        deployment,
        path,
        "app=desktop&code=one-time&state=bound",
    )
    .await
    .unwrap();
    validate_callback(
        &tx,
        &f.app,
        deployment,
        path,
        "app=desktop&error=access_denied&state=bound",
    )
    .await
    .unwrap();
    for query in [
        "code=x&state=bound",
        "app=desktop&code=x&state=bound&next=https%3A%2F%2Fevil.example",
        "app=desktop&code=x&state=bound&%73tate=again",
        "app=desktop&error=access_denied&code=x&state=bound",
        "app=desktop&code=x&state=bound&extra=x",
    ] {
        assert!(validate_callback(&tx, &f.app, deployment, path, query)
            .await
            .is_err());
    }
    assert!(validate_callback(
        &tx,
        &f.app,
        deployment,
        "/oauth/authorize",
        "app=desktop&code=x&state=bound"
    )
    .await
    .is_err());
    let policy = csp(&tx, &f.app, deployment, &signed).await.unwrap();
    assert!(policy.contains(&format!(
        "connect-src {};",
        f.app.config.text("server", "public_origin")
    )));
    assert!(policy.contains("frame-src https://*.plugins.example;"));
    assert!(
        !policy.contains(&deployment.to_string())
            && !policy.contains(&deployment.simple().to_string())
    );
    assert!(!policy.contains("unsafe-inline") && !policy.contains("unsafe-eval"));
    tx.commit().await.unwrap();
    assert_eq!(
        redirect(&f.app).await.unwrap(),
        format!("{origin}/ui/org.xelab.webui/")
    );
    c.execute(
        "UPDATE clients SET status='disabled' WHERE spec->>'plugin_id'='org.xelab.webui'",
        &[],
    )
    .await
    .unwrap();
    assert!(redirect(&f.app).await.is_err());
    assert!(frame_ancestors(&c, &f.app, contribution)
        .await
        .unwrap()
        .is_empty());
    // Historical metadata remains valid while live clients and runtime trust
    // are absent; restore will leave the selected deployment unavailable.
    validate_snapshot(&c).await.unwrap();
    c.execute(
        "UPDATE deployments SET spec='{\"enabled\":false}',phase='disabled' WHERE id=$1",
        &[&deployment],
    )
    .await
    .unwrap();
    assert!(redirect(&f.app).await.is_err());
    assert!(
        f.app.db.check().await.is_ok(),
        "the CLI/core API remain independent of the WebUI"
    );
}
