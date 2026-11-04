//! Actual HTTP/PostgreSQL contract checks. Installation/observations here are
//! metadata fixtures, not OCI health or runtime stop proofs.
use super::*;
use crate::{backups::tests::Fixture, contracts::validate};
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::Request,
};
use tower::ServiceExt;

#[tokio::test]
async fn actual_postgres_deployment_operations_return_complete_objects_and_respect_intent() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let c = f.app.db.connect().await.unwrap();
    let installed = c
        .query_one("SELECT id,publisher_id FROM installations LIMIT 1", &[])
        .await
        .unwrap();
    let installation: Uuid = installed.get(0);
    let mut manifest: Value =
        serde_json::from_str(include_str!("../tests/fixtures/secret-manifest.json")).unwrap();
    manifest["publisher_id"] = json!(installed.get::<_, Uuid>(1));
    manifest["requested_rules"] = json!([]);
    c.execute(
        "UPDATE installations SET plugin_id='org.xelab.echo-python',manifest=$2 WHERE id=$1",
        &[&installation, &manifest],
    )
    .await
    .unwrap();
    let spec = json!({"installation_id":installation,"display_name":"operation result contract","enabled":false,"limits":manifest["minimum_resources"],"config":{},"volume_ids":[]});
    let request = Request::builder()
        .method("POST")
        .uri("/api/platform/v1/deployments")
        .header("authorization", format!("Bearer {}", f.token))
        .header("content-type", "application/json")
        .header("idempotency-key", "complete-deployment-operation");
    let mut request = request
        .body(Body::from(serde_json::to_vec(&spec).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:30000".parse::<std::net::SocketAddr>().unwrap(),
    ));
    let response = crate::api::router(f.app.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let created: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4194304).await.unwrap()).unwrap();
    assert_eq!(created["state"], "succeeded");
    validate("Deployment", &created["result"]).unwrap();
    assert_eq!(created["result"]["id"], created["resource_id"]);
    assert_eq!(created["result"]["spec"]["replicas"], 1);
    assert_eq!(created["result"]["phase"], "disabled");
    let deployment: Uuid = created["resource_id"].as_str().unwrap().parse().unwrap();
    let owner: Uuid = c
        .query_one(
            "SELECT owner_id FROM deployments WHERE id=$1",
            &[&deployment],
        )
        .await
        .unwrap()
        .get(0);
    for enabled in [true, false] {
        let operation = db::operation(
            &c,
            owner,
            "setDeploymentEnabled",
            Some(deployment.to_string()),
        )
        .await
        .unwrap();
        // The previous opposite intent must not complete this observation.
        finish_deployment_operations(&c, deployment, enabled)
            .await
            .unwrap();
        assert_eq!(
            db::operation_value(&c, operation).await.unwrap()["state"],
            "pending"
        );
        let phase = if enabled { "ready" } else { "disabled" };
        c.execute("UPDATE deployments SET spec=jsonb_set(spec,'{enabled}',$2),phase=$3,observed_generation=3,revision=revision+1,updated_at=now() WHERE id=$1", &[&deployment,&json!(enabled),&phase]).await.unwrap();
        finish_deployment_operations(&c, deployment, enabled)
            .await
            .unwrap();
        let completed = db::operation_value(&c, operation).await.unwrap();
        assert_eq!(completed["state"], "succeeded");
        validate("Deployment", &completed["result"]).unwrap();
        assert_eq!(
            completed["result"],
            crate::api::read_object(&c, "deployments", deployment, "Deployment")
                .await
                .unwrap()
        );
        assert_eq!(completed["result"]["spec"]["enabled"], enabled);
        assert_eq!(completed["result"]["observed_generation"], 3);
        assert_eq!(completed["result"]["phase"], phase);
        finish_deployment_operations(&c, deployment, !enabled)
            .await
            .unwrap();
        assert_eq!(
            db::operation_value(&c, operation).await.unwrap(),
            completed,
            "late observations cannot rewrite a terminal result"
        );
    }
}
