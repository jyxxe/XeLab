use super::*;

#[test]
fn output_limit_counts_serialized_utf8_and_escapes() {
    let value = json!({"text": "含引号的 \"JSON\"\n"});
    let wire = serde_json::to_vec(&value).unwrap();
    let limit = wire.len() as u64;
    check_output_bytes(&value, limit).unwrap();
    assert_eq!(
        check_output_bytes(&value, limit - 1).unwrap_err().code,
        "INVALID_ARGUMENT"
    );
}

#[tokio::test]
async fn output_uses_the_pinned_deployments_limit_and_signed_http_skill_schema() {
    let Ok(url) = std::env::var("XELAB_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: task result binding requires PostgreSQL");
        return;
    };
    let mut client = crate::db::Db::new(url).unwrap().connect().await.unwrap();
    let tx = client.transaction().await.unwrap();
    tx.batch_execute(
        "CREATE TEMP TABLE task_steps(id uuid, provider_deployment_id uuid);
         CREATE TEMP TABLE deployments(id uuid, installation_id uuid);
         CREATE TEMP TABLE installations(id uuid, manifest jsonb);
         CREATE TEMP TABLE capability_contracts(name text,version text,schema_digest text,spec jsonb);
         CREATE TEMP TABLE skills(id uuid,installation_id uuid,definition jsonb);",
    ).await.unwrap();
    let step = Uuid::new_v4();
    let deployment = Uuid::new_v4();
    let installation = Uuid::new_v4();
    let reference = json!({"name":"tool.echo","version":"1.0.0","schema_digest":"digest"});
    let registered =
        json!({"contract":reference,"output_schema":{"type":"string"},"maximum_output_bytes":128});
    let mut pinned = registered.clone();
    pinned["maximum_output_bytes"] = json!(8);
    tx.execute(
        "INSERT INTO capability_contracts VALUES('tool.echo','1.0.0','digest',$1)",
        &[&registered],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO installations VALUES($1,$2)",
        &[&installation, &json!({"capabilities":[pinned]})],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO deployments VALUES($1,$2)",
        &[&deployment, &installation],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO task_steps VALUES($1,$2)",
        &[&step, &deployment],
    )
    .await
    .unwrap();
    let node = json!({"contract":reference});
    validate_output(&tx, step, &node, &json!("123456"))
        .await
        .unwrap();
    assert_eq!(
        validate_output(&tx, step, &node, &json!("1234567"))
            .await
            .unwrap_err()
            .code,
        "INVALID_ARGUMENT"
    );

    let skill = Uuid::new_v4();
    let http =
        json!({"target":{"kind":"http"},"output_schema":{"type":"string"},"stream_item_schema":{}});
    tx.execute(
        "INSERT INTO skills VALUES($1,$2,$3)",
        &[&skill, &installation, &http],
    )
    .await
    .unwrap();
    let node = json!({"skill_id":skill});
    validate_output(&tx, step, &node, &json!("result"))
        .await
        .unwrap();
    assert_eq!(
        validate_output(&tx, step, &node, &json!(42))
            .await
            .unwrap_err()
            .code,
        "INVALID_ARGUMENT"
    );
    let too_large = Value::String("x".repeat(1024 * 1024));
    assert_eq!(
        validate_output(&tx, step, &node, &too_large)
            .await
            .unwrap_err()
            .code,
        "INVALID_ARGUMENT"
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn public_step_timestamps_are_identical_across_session_time_zones() {
    let Ok(url) = std::env::var("XELAB_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: task step projection requires PostgreSQL");
        return;
    };
    let mut client = crate::db::Db::new(url).unwrap().connect().await.unwrap();
    let tx = client.transaction().await.unwrap();
    tx.batch_execute(
        "CREATE TEMP TABLE task_steps AS SELECT gen_random_uuid() AS id,gen_random_uuid() AS task_id,
                 'done'::text AS step_key,NULL::uuid AS provider_deployment_id,1 AS attempt_count,
                 'succeeded'::text AS state,2 AS revision,now() AS created_at,now() AS updated_at;
         SET LOCAL TIME ZONE 'Asia/Shanghai'",
    ).await.unwrap();
    let step = tx
        .query_one("SELECT id FROM task_steps", &[])
        .await
        .unwrap()
        .get(0);
    let original = read_step(&tx, step).await.unwrap();
    assert!(original["updated_at"].as_str().unwrap().ends_with('Z'));
    tx.batch_execute("SET LOCAL TIME ZONE 'America/New_York'")
        .await
        .unwrap();
    assert_eq!(read_step(&tx, step).await.unwrap(), original);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn unknown_without_error_overrides_or_survives_a_failed_sibling() {
    let Ok(url) = std::env::var("XELAB_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: parallel outcome aggregation requires PostgreSQL");
        return;
    };
    let mut client = crate::db::Db::new(url).unwrap().connect().await.unwrap();
    let tx = client.transaction().await.unwrap();
    tx.batch_execute(
        "CREATE TEMP TABLE tasks(id uuid, state text, error jsonb, output jsonb, revision integer, updated_at timestamptz);
         CREATE TEMP TABLE task_steps(id uuid,task_id uuid,state text);
         CREATE TEMP TABLE attempts(step_id uuid,state text);",
    ).await.unwrap();
    for outcomes in [["failed", "unknown"], ["unknown", "failed"]] {
        let task = Uuid::new_v4();
        tx.execute(
            "INSERT INTO tasks(id,state,revision) VALUES($1,'running',1)",
            &[&task],
        )
        .await
        .unwrap();
        let steps = [Uuid::new_v4(), Uuid::new_v4()];
        for step in &steps {
            tx.execute(
                "INSERT INTO task_steps VALUES($1,$2,'running')",
                &[step, &task],
            )
            .await
            .unwrap();
            tx.execute("INSERT INTO attempts VALUES($1,'leased')", &[step])
                .await
                .unwrap();
        }
        for (index, (step, state)) in steps.iter().zip(outcomes).enumerate() {
            tx.execute(
                "UPDATE attempts SET state=$2 WHERE step_id=$1",
                &[step, &state],
            )
            .await
            .unwrap();
            tx.execute(
                "UPDATE task_steps SET state=$2 WHERE id=$1",
                &[step, &state],
            )
            .await
            .unwrap();
            let error = if state == "failed" {
                Error::invalid("Worker failed").detail()
            } else {
                Value::Null
            };
            aggregate(&tx, task, *step, state, &error).await.unwrap();
            let row = tx
                .query_one(
                    "SELECT state,error IS NULL,output IS NULL FROM tasks WHERE id=$1",
                    &[&task],
                )
                .await
                .unwrap();
            assert_eq!(
                row.get::<_, &str>(0),
                if index == 0 { "running" } else { "unknown" }
            );
            assert_eq!(row.get::<_, bool>(1), index != 0 || state == "unknown");
            assert!(row.get::<_, bool>(2));
        }
    }
    tx.rollback().await.unwrap();
}
