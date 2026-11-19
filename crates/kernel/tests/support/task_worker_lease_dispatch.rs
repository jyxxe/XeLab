//! Dispatch requests through the public broker and Skill schemas.
use super::{leased, spec, stored, submit, succeed};
use crate::Fixture;
use serde_json::json;
use uuid::Uuid;
use xelab_kernel::{
    contracts::{id, validate},
    task_engine::prepare_dispatch,
};

#[tokio::test]
async fn maps_inputs_in_dependency_order_and_preserves_the_admitted_targets_and_keys() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let provider = f.deployment(true).await;
    let skill = f.register_skill().await;
    let mut spec = spec(&f, 600_000);
    let root = &mut spec["steps"][0];
    root["input_mapping"] = json!({"message":{"task_input":"/a~1b/~0/1"}});
    let mut middle = root.clone();
    middle["id"] = json!("middle");
    middle["timeout_ms"] = json!(60_000);
    middle["depends_on"] = json!(["echo"]);
    middle["provider_deployment_id"] = json!(provider);
    middle["input_mapping"] = json!({"message":{"literal":"fixed value"}});
    let sink = json!({"id":"sink","depends_on":["middle"],"skill_id":skill,"timeout_ms":10_000,
        "input_mapping":{"message":{"step_output":{"step":"echo","pointer":"/message"}}}});
    spec["steps"].as_array_mut().unwrap().extend([middle, sink]);
    let task = submit(
        &f,
        &spec,
        json!({"a/b":{"~":["decoy","from task"]}}),
        "mapped",
    )
    .await;
    // A fresh provider search for this Skill is now ambiguous.
    f.deployment(true).await;
    let db = f.db.connect().await.unwrap();
    for (step, message, schema, target, pinned, timeout_ms) in [
        ("echo", "from task", "InvokeRequest", None, None, 300_000),
        (
            "middle",
            "fixed value",
            "InvokeRequest",
            None,
            Some(provider),
            60_000,
        ),
        (
            "sink",
            "output of echo",
            "SkillInvokeRequest",
            Some(skill),
            Some(provider),
            10_000,
        ),
    ] {
        let (lease, attempt) = leased(&f).await;
        let before = stored(&db, lease.attempt_id).await;
        let prepared = prepare_dispatch(&f.db, &f.auth, &lease).await.unwrap();
        assert_eq!(
            stored(&db, lease.attempt_id).await,
            before,
            "prepare is read-only"
        );
        validate(schema, &prepared.request).unwrap();
        assert_eq!(prepared.request["timeout_ms"], timeout_ms);
        assert_eq!(prepared.skill_id, target);
        assert_eq!(prepared.provider_deployment_id, pinned);
        assert_eq!(
            prepared.request.get("provider_deployment_id"),
            pinned.map(|p| json!(p)).as_ref()
        );
        assert_eq!(
            prepared.request.get("contract"),
            target.is_none().then_some(&f.contract)
        );
        assert_eq!(prepared.request["input"], json!({"message":message}));
        assert_eq!(
            prepared.request["idempotency_key"],
            attempt["idempotency_key"]
        );
        assert_ne!(prepared.request["idempotency_key"], attempt["execution_id"]);
        let repeated = prepare_dispatch(&f.db, &f.auth, &lease).await.unwrap();
        assert_eq!(repeated.request, prepared.request);
        assert_eq!(repeated.deadline_at, prepared.deadline_at);
        if step == "sink" {
            // Keep the committed output in place but withdraw its completion marker.
            let upstream = db.query_one(
                "UPDATE task_steps SET state='running' WHERE task_id=$1 AND step_key='echo' RETURNING id",
                &[&task],
            ).await.unwrap().get::<_, Uuid>(0);
            let error = prepare_dispatch(&f.db, &f.auth, &lease)
                .await
                .expect_err("a transitive predecessor must still be succeeded");
            assert_eq!(error.code, "INVALID_ARGUMENT", "{error}");
            db.execute(
                "UPDATE task_steps SET state='succeeded' WHERE id=$1",
                &[&upstream],
            )
            .await
            .unwrap();
        }
        let committed = succeed(&f, &lease, &format!("output of {step}")).await;
        assert_eq!(committed["step_key"], step);
    }
}

#[tokio::test]
async fn missing_input_or_output_pointer_is_an_argument_error() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let mut spec = spec(&f, 60_000);
    for (step, leaf) in [
        ("from-task", json!({"task_input":"/absent"})),
        (
            "from-output",
            json!({"step_output":{"step":"echo","pointer":"/absent"}}),
        ),
    ] {
        spec["steps"].as_array_mut().unwrap().push(json!({
            "id":step,"contract":f.contract,"depends_on":["echo"],"timeout_ms":60_000,
            "input_mapping":{"message":leaf}
        }));
    }
    submit(
        &f,
        &spec,
        json!({"message":"exists elsewhere"}),
        "missing-paths",
    )
    .await;
    let (root, _) = leased(&f).await;
    succeed(&f, &root, "real predecessor output").await;

    let mut steps = std::collections::HashSet::new();
    for _ in 0..2 {
        let (lease, attempt) = leased(&f).await;
        assert!(steps.insert(id(&attempt, "step_id").unwrap()));
        let before = stored(&db, lease.attempt_id).await;
        let error = prepare_dispatch(&f.db, &f.auth, &lease)
            .await
            .expect_err("missing pointer");
        assert_eq!(error.code, "INVALID_ARGUMENT", "{error}");
        assert_eq!(stored(&db, lease.attempt_id).await, before);
    }
}
