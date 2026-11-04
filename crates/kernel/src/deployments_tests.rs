//! Actual PostgreSQL and public management requests. Enable publishes the
//! existing Controller intent; no OCI launch or host admission is claimed here.
use super::*;
use crate::{backups::tests::Fixture, contracts, plugin_http::transport::StaticOrigin};
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::Request,
};
use tower::ServiceExt;

struct DeploymentFixture {
    base: Fixture,
    deployment: Uuid,
    installation: Uuid,
    owner: Context,
    spec: Value,
    _artifact: tempfile::TempDir,
}

impl DeploymentFixture {
    async fn new() -> Option<Self> {
        let mut base = Fixture::new().await?;
        let mut value = base.app.config.value.clone();
        value["ui_gateway"] =
            json!({"base_domain":"deployments.example.org","public_port":443,"shell_origins":[]});
        std::sync::Arc::get_mut(&mut base.app).unwrap().config =
            crate::config::Config::from_value(value).unwrap();
        let c = base.app.db.connect().await.unwrap();
        let owner = base
            .app
            .auth
            .authenticate(
                &c,
                &base.token,
                "127.0.0.1".parse().unwrap(),
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        let publisher = Uuid::new_v4();
        let (artifact, mut manifest, raw) =
            crate::http_contracts::tests::fixture_manifest(publisher);
        // The signed fixture accepts a public Client identifier or an inert
        // disabled bootstrap placeholder. It contains no core credential.
        manifest["config_schema"] = json!({"type":"object","properties":{"client_id":{"anyOf":[{"type":"string","pattern":"^xelab-[a-f0-9]{32}$"},{"type":"null"}]}},"required":["client_id"],"additionalProperties":false});
        crate::artifact::validate_profile(&manifest).unwrap();
        let installation = Uuid::new_v4();
        c.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'Deployment metadata publisher',$2,'fixture-only','trusted',now(),now())", &[&publisher,&publisher.to_string()]).await.unwrap();
        let hash = contracts::digest(&serde_json::to_vec(&manifest).unwrap());
        c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$5,$6,'installed',now(),now())", &[&installation,&manifest["plugin_id"].as_str().unwrap(),&manifest["version"].as_str().unwrap(),&publisher,&hash,&manifest]).await.unwrap();
        crate::http_contracts::store(&c, installation, &manifest, Some(&raw))
            .await
            .unwrap();
        let spec = json!({"installation_id":installation,"display_name":"disabled browser bootstrap","enabled":false,"replicas":1,"limits":{"cpu_millis":200,"memory_bytes":134217728,"ephemeral_bytes":1048576,"pids":64},"config":{"client_id":null},"volume_ids":[]});
        let mut f = Self {
            base,
            deployment: Uuid::nil(),
            installation,
            owner,
            spec,
            _artifact: artifact,
        };
        let created = f
            .request(
                "POST",
                "/api/platform/v1/deployments",
                f.spec.clone(),
                "create-disabled",
                None,
                None,
            )
            .await;
        assert_eq!(created.0, 202, "{}", created.1);
        assert_eq!(created.1["state"], "succeeded");
        f.deployment = created.1["resource_id"].as_str().unwrap().parse().unwrap();
        Some(f)
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Value,
        key: &str,
        revision: Option<i64>,
        token: Option<&str>,
    ) -> (u16, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(
                "host",
                crate::plugin_http_gateway::core_authority(&self.base.app.config).unwrap(),
            )
            .header(
                "authorization",
                format!("Bearer {}", token.unwrap_or(&self.base.token)),
            )
            .header("idempotency-key", key);
        if let Some(revision) = revision {
            request = request.header("if-match", format!("\"rev-{revision}\""));
        }
        let bytes = if body.is_null() {
            Vec::new()
        } else {
            request = request.header("content-type", "application/json");
            serde_json::to_vec(&body).unwrap()
        };
        let mut request = request.body(Body::from(bytes)).unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:50400".parse::<std::net::SocketAddr>().unwrap(),
        ));
        let response = crate::api::router(self.base.app.clone())
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status().as_u16();
        let bytes = to_bytes(response.into_body(), 1048576).await.unwrap();
        (
            status,
            if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap()
            },
        )
    }
    async fn update(&self, spec: Value, key: &str, revision: i64) -> (u16, Value) {
        self.request(
            "PUT",
            &format!("/api/platform/v1/deployments/{}", self.deployment),
            json!({"spec":spec}),
            key,
            Some(revision),
            None,
        )
        .await
    }
    async fn read(&self) -> Value {
        let result = self
            .request(
                "GET",
                &format!("/api/platform/v1/deployments/{}", self.deployment),
                Value::Null,
                "read",
                None,
                None,
            )
            .await;
        assert_eq!(result.0, 200, "{}", result.1);
        result.1
    }
}

#[tokio::test]
async fn actual_postgres_disabled_update_bootstraps_a_registered_client_and_preserves_enable_flow()
{
    let Some(f) = DeploymentFixture::new().await else {
        return;
    };
    let c = f.base.app.db.connect().await.unwrap();
    let origin = StaticOrigin::for_deployment(&f.base.app.config, f.deployment, None)
        .unwrap()
        .origin()
        .to_owned();
    let client=f.request("POST","/api/platform/v1/clients",json!({"name":"Actual disabled bootstrap client","plugin_id":"org.xelab.http-echo","redirect_uris":[format!("{origin}/ui/org.xelab.http-echo/callback")],"allowed_origins":[origin],"maximum_rules":[{"action":"account.read","resource":{"kind":"account","match":"ids","ids":[f.owner.subject]}}]}),"register-ui-client",None,None).await;
    assert_eq!(client.0, 201, "{}", client.1);
    let before = c
        .query_one(
            "SELECT reserved_cpu_millis,reserved_memory_bytes FROM node_budget",
            &[],
        )
        .await
        .unwrap();
    let mut wanted = f.spec.clone();
    wanted["display_name"] = json!("configured browser client");
    wanted["config"]["client_id"] = client.1["client_id"].clone();
    wanted["limits"]["cpu_millis"] = json!(200.0);
    wanted["replicas"] = json!(1.0);
    let updated = f.update(wanted.clone(), "configure-registered-ui", 1).await;
    assert_eq!(updated.0, 202, "{}", updated.1);
    assert_eq!(updated.1["kind"], "updateDeployment");
    assert_eq!(updated.1["state"], "succeeded");
    assert_eq!(updated.1["resource_id"], json!(f.deployment));
    let deployment = f.read().await;
    assert_eq!(
        deployment["spec"]["config"]["client_id"],
        client.1["client_id"]
    );
    assert_eq!(deployment["spec"]["limits"]["cpu_millis"], 200);
    assert_eq!(deployment["spec"]["enabled"], false);
    assert_eq!(deployment["revision"], 2);
    let after = c
        .query_one(
            "SELECT reserved_cpu_millis,reserved_memory_bytes FROM node_budget",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        (before.get::<_, i64>(0), before.get::<_, i64>(1)),
        (after.get::<_, i64>(0), after.get::<_, i64>(1))
    );
    assert_eq!(
        f.update(wanted.clone(), "configure-registered-ui", 1).await,
        updated,
        "exact retry returns the original persisted operation"
    );
    wanted["display_name"] = json!("different business intent");
    let conflict = f.update(wanted.clone(), "configure-registered-ui", 1).await;
    assert_eq!(conflict.0, 409);
    assert_eq!(conflict.1["error"]["code"], "IDEMPOTENCY_CONFLICT");
    assert_eq!(f.update(wanted, "new-key-old-etag", 1).await.0, 412);
    let approved = f
        .request(
            "PUT",
            &format!("/api/platform/v1/deployments/{}/approval", f.deployment),
            json!({"rules":[],"egress":[]}),
            "approve-configured-ui",
            Some(2),
            None,
        )
        .await;
    assert_eq!(approved.0, 200, "{}", approved.1);
    let enabled = f
        .request(
            "PUT",
            &format!("/api/platform/v1/deployments/{}/enabled", f.deployment),
            json!({"enabled":true}),
            "enable-configured-ui",
            Some(3),
            None,
        )
        .await;
    assert_eq!(enabled.0, 202, "{}", enabled.1);
    let desired = f.read().await;
    assert_eq!(desired["spec"]["enabled"], true);
    assert_eq!(desired["phase"], "reconciling");
    assert_eq!(
        desired["spec"]["config"]["client_id"],
        client.1["client_id"]
    );
    assert_eq!(
        f.update(desired["spec"].clone(), "cannot-live-reconfigure", 4)
            .await
            .0,
        409
    );
}

#[tokio::test]
async fn actual_postgres_deployment_update_rejects_protected_fields_limits_and_foreign_accounts() {
    let Some(f) = DeploymentFixture::new().await else {
        return;
    };
    for fault in [
        "installation",
        "volumes",
        "enabled",
        "replicas",
        "minimum",
        "schema",
        "unknown",
    ] {
        let mut wanted = f.spec.clone();
        match fault {
            "installation" => wanted["installation_id"] = json!(Uuid::new_v4()),
            "volumes" => wanted["volume_ids"] = json!([Uuid::new_v4()]),
            "enabled" => wanted["enabled"] = json!(true),
            "replicas" => wanted["replicas"] = json!(2.0),
            "minimum" => wanted["limits"]["cpu_millis"] = json!(99),
            "schema" => wanted["config"]["client_id"] = json!(42),
            "unknown" => wanted["untrusted_address"] = json!("https://invalid.example.org"),
            _ => unreachable!(),
        }
        let response = f.update(wanted, &format!("reject-{fault}"), 1).await;
        assert_eq!(
            response.0,
            if matches!(fault, "installation" | "volumes") {
                409
            } else {
                400
            },
            "{fault}: {}",
            response.1
        );
    }
    let c = f.base.app.db.connect().await.unwrap();
    let guest = Uuid::new_v4();
    let password = "deployment foreign account password";
    c.execute("INSERT INTO accounts(id,kind,status,display_name,password_hash,created_at,updated_at) VALUES($1,'user','active','Foreign configurator',$2,now(),now())", &[&guest,&auth::password_hash(password).unwrap()]).await.unwrap();
    c.execute(
        "INSERT INTO account_closure(ancestor_id,descendant_id,depth) VALUES($1,$1,0)",
        &[&guest],
    )
    .await
    .unwrap();
    let session = f
        .base
        .app
        .auth
        .login(&c, guest, password.into(), f.owner.peer, Uuid::new_v4())
        .await
        .unwrap();
    let foreign = f
        .request(
            "PUT",
            &format!("/api/platform/v1/deployments/{}", f.deployment),
            json!({"spec":f.spec}),
            "foreign-configure",
            Some(1),
            Some(session["access_token"].as_str().unwrap()),
        )
        .await;
    assert_eq!(foreign.0, 403, "{}", foreign.1);
    assert_eq!(f.read().await["revision"], 1);
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM operations WHERE kind='updateDeployment'",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    assert_eq!(
        f.installation,
        contracts::id(&f.read().await["spec"], "installation_id").unwrap()
    );
}

#[tokio::test]
async fn actual_postgres_deployment_update_fails_closed_on_runtime_leases_and_never_waits_on_controller(
) {
    let Some(f) = DeploymentFixture::new().await else {
        return;
    };
    let c = f.base.app.db.connect().await.unwrap();
    let fence = db::deployment_fence(&f.base.app.db, f.deployment)
        .await
        .unwrap()
        .unwrap();
    let blocked = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.update(f.spec.clone(), "controller-active", 1),
    )
    .await
    .expect("configuration must try the runtime fence rather than wait behind Controller");
    assert_eq!(blocked.0, 409, "{}", blocked.1);
    drop(fence);
    let instance = Uuid::new_v4();
    c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,runtime_id,created_at,updated_at) VALUES($1,$2,1,0,'stopped','unknown','{}',$3,now(),now())", &[&instance,&f.deployment,&instance.to_string()]).await.unwrap();
    assert_eq!(
        f.update(f.spec.clone(), "stopped-runtime-still-present", 1)
            .await
            .0,
        409
    );
    c.execute(
        "UPDATE instances SET runtime_id=NULL WHERE id=$1",
        &[&instance],
    )
    .await
    .unwrap();
    let call = Uuid::new_v4();
    c.execute("INSERT INTO calls(id,owner_id,credential_id,provider_instance_id,contract,input_digest,state,deadline_at,created_at) VALUES($1,$2,$3,$4,'{}','explicit-resource-lease-fixture','unknown',now()+interval '1 minute',now())", &[&call,&f.owner.subject,&f.owner.credential,&instance]).await.unwrap();
    c.execute("INSERT INTO reservations(id,account_id,kind,status,created_at) VALUES($1,$2,'call','active',now())", &[&call,&f.owner.subject]).await.unwrap();
    assert_eq!(
        f.update(f.spec.clone(), "unknown-call-retains-slot", 1)
            .await
            .0,
        409
    );
    c.execute(
        "UPDATE reservations SET status='released' WHERE id=$1",
        &[&call],
    )
    .await
    .unwrap();
    let lease = Uuid::new_v4();
    c.execute("INSERT INTO plugin_http_requests(id,instance_id,generation,expires_at,state) VALUES($1,$2,1,now()-interval '1 minute','unknown')", &[&lease,&instance]).await.unwrap();
    assert_eq!(
        f.update(f.spec.clone(), "expired-static-still-unknown", 1)
            .await
            .0,
        409
    );
    c.execute("DELETE FROM plugin_http_requests WHERE id=$1", &[&lease])
        .await
        .unwrap();
    let held = db::operation(
        &c,
        f.owner.subject,
        "createBackup",
        Some(f.deployment.to_string()),
    )
    .await
    .unwrap();
    c.execute(
        "INSERT INTO deployment_storage_holds(deployment_id,operation_id) VALUES($1,$2)",
        &[&f.deployment, &held],
    )
    .await
    .unwrap();
    assert_eq!(
        f.update(f.spec.clone(), "storage-still-held", 1).await.0,
        409
    );
    c.execute(
        "DELETE FROM deployment_storage_holds WHERE deployment_id=$1",
        &[&f.deployment],
    )
    .await
    .unwrap();
    let updated = f.update(f.spec.clone(), "fully-stopped-configure", 1).await;
    assert_eq!(updated.0, 202, "{}", updated.1);
    assert_eq!(updated.1["result"]["phase"], "disabled");
}
