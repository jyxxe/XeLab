use super::*;
use crate::db::Db;

async fn database() -> Option<tokio_postgres::Client> {
    let Ok(dsn) = std::env::var("XELAB_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: dispatch mapping requires PostgreSQL");
        return None;
    };
    Some(Db::new(dsn).unwrap().connect().await.unwrap())
}

#[tokio::test]
async fn postgres_mapping_resolves_rfc6901_against_only_the_selected_task() {
    let Some(mut db) = database().await else {
        return;
    };
    let tx = db.transaction().await.unwrap();
    tx.batch_execute(
        "CREATE TEMP TABLE tasks(id uuid,input jsonb);
         CREATE TEMP TABLE task_steps(task_id uuid,step_key text,state text,output jsonb);",
    )
    .await
    .unwrap();
    let task = Uuid::new_v4();
    let value = json!({"a/b":{"~":["zero",null]},"~1":"tilde-one","":{"+1":0,"-1":1,"01":2}});
    tx.execute("INSERT INTO tasks VALUES ($1,$2)", &[&task, &value])
        .await
        .unwrap();
    tx.execute(
        "INSERT INTO task_steps VALUES ($1,'a','succeeded','null'), ($2,'a','succeeded','42')",
        &[&task, &Uuid::new_v4()],
    )
    .await
    .unwrap();

    let opaque = json!({"task_input":"/missing","step_output":{"step":"forged"}});
    let mapping = json!({
        "literal":{"literal":opaque}, "empty":{"literal":null}, "root":{"task_input":""},
        "null":{"task_input":"/a~1b/~0/1"},"escaped":{"task_input":"/~01"},
        "plus":{"task_input":"//+1"},"negative":{"task_input":"//-1"},"leading":{"task_input":"//01"},
        "result":{"step_output":{"step":"a","pointer":""}}
    });
    assert_eq!(
        mapping::resolve(&tx, task, &mapping).await.unwrap(),
        json!({"literal":opaque,"empty":null,"root":value,"null":null,"escaped":"tilde-one",
            "plus":0,"negative":1,"leading":2,"result":null})
    );
    for token in ["+1", "01", "-1", "-", "2", ""] {
        let mapping = json!({"value":{"task_input":format!("/a~1b/~0/{token}")}});
        assert_eq!(
            mapping::resolve(&tx, task, &mapping)
                .await
                .unwrap_err()
                .code,
            "INVALID_ARGUMENT"
        );
    }
    let output = json!({"x":{"step_output":{"step":"a","pointer":""}}});
    for change in ["state='running'", "state='succeeded',output=NULL"] {
        tx.execute(
            &format!("UPDATE task_steps SET {change} WHERE task_id=$1"),
            &[&task],
        )
        .await
        .unwrap();
        assert_eq!(
            mapping::resolve(&tx, task, &output).await.unwrap_err().code,
            "INVALID_ARGUMENT"
        );
    }
}

#[tokio::test]
async fn postgres_repeated_root_expansion_is_bounded() {
    let Some(mut db) = database().await else {
        return;
    };
    let tx = db.transaction().await.unwrap();
    let task = Uuid::new_v4();
    tx.batch_execute("CREATE TEMP TABLE tasks(id uuid,input jsonb)")
        .await
        .unwrap();
    tx.execute(
        "INSERT INTO tasks VALUES ($1,to_jsonb(repeat('x',1048576)))",
        &[&task],
    )
    .await
    .unwrap();
    let mapping = json!({"a":{"task_input":""},"b":{"task_input":""},
                         "c":{"task_input":""},"d":{"task_input":""}});
    // Four 1 MiB strings plus their keys, quotes and punctuation exceed 4 MiB.
    assert_eq!(
        mapping::resolve(&tx, task, &mapping)
            .await
            .unwrap_err()
            .code,
        "RESOURCE_EXHAUSTED"
    );
}

#[tokio::test]
async fn postgres_narrow_pointers_can_span_more_than_64_mib_of_sources() {
    let Some(mut db) = database().await else {
        return;
    };
    let tx = db.transaction().await.unwrap();
    let task = Uuid::new_v4();
    tx.batch_execute(
        "CREATE TEMP TABLE task_steps(task_id uuid,step_key text,state text,output jsonb)",
    )
    .await
    .unwrap();
    tx.execute("INSERT INTO task_steps
                SELECT $1,'step'||i,'succeeded',jsonb_build_object('index',i,'data',repeat('x',14*1024*1024))
                  FROM generate_series(1,5) i", &[&task]).await.unwrap();
    let mut mapping = json!({});
    let mut expected = json!({});
    for i in 1..=5 {
        let key = format!("step{i}");
        mapping[&key] = json!({"step_output":{"step":key,"pointer":"/index"}});
        expected[&key] = json!(i);
    }
    assert_eq!(
        mapping::resolve(&tx, task, &mapping).await.unwrap(),
        expected
    );
}

#[tokio::test]
async fn postgres_build_checks_ancestry_and_distinguishes_the_two_broker_envelopes() {
    let Some(mut db) = database().await else {
        return;
    };
    let tx = db.transaction().await.unwrap();
    tx.batch_execute(
        "CREATE TEMP TABLE tasks(id uuid,workflow_id uuid);
         CREATE TEMP TABLE workflows(id uuid,spec jsonb);
         CREATE TEMP TABLE task_steps(id uuid,task_id uuid,step_key text,provider_deployment_id uuid,
                                      fence bigint,attempt_count integer,state text,output jsonb);
         CREATE TEMP TABLE attempts(step_id uuid,fence bigint,number integer,idempotency_key text);
         CREATE TEMP TABLE skills(id uuid,definition jsonb);",
    ).await.unwrap();
    let task = Uuid::new_v4();
    let workflow = Uuid::new_v4();
    let step = Uuid::new_v4();
    let provider = Uuid::new_v4();
    let skill = Uuid::new_v4();
    let contract = json!({"name":"tool.echo","version":"1.0.0","schema_digest":crate::contracts::digest(b"echo")});
    let node = |id, deps, mapping| {
        json!({"id":id,"depends_on":deps,"input_mapping":mapping,
                                         "timeout_ms":600000,"contract":contract})
    };
    let mut spec = json!({"name":"pinned","input_schema":{},"steps":[
        node("root",json!([]),json!({})),node("middle",json!(["root"]),json!({})),
        node("side",json!([]),json!({})),
        node("current",json!(["middle"]),json!({"x":{"step_output":{"step":"root","pointer":"/0"}}}))
    ]});
    tx.execute("INSERT INTO workflows VALUES ($1,$2)", &[&workflow, &spec])
        .await
        .unwrap();
    tx.execute("INSERT INTO tasks VALUES ($1,$2)", &[&task, &workflow])
        .await
        .unwrap();
    tx.execute(
        "INSERT INTO task_steps VALUES ($1,$2,'current',$3,2,2,'running',NULL),
                                             (NULL,$2,'root',NULL,1,1,'succeeded','[17]'),
                                             (NULL,$2,'side',NULL,1,1,'succeeded','[18]')",
        &[&step, &task, &provider],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO attempts VALUES ($1,1,1,'old'), ($1,2,2,'stable')",
        &[&step],
    )
    .await
    .unwrap();
    let deadline = Utc::now() + chrono::Duration::seconds(80);
    let input = build(&tx, step, deadline).await.unwrap();
    assert_eq!(input.deadline_at, deadline);
    let earlier = deadline - chrono::Duration::seconds(79);
    let repeated = build(&tx, step, earlier).await.unwrap();
    assert_eq!(repeated.deadline_at, earlier);
    assert_eq!(repeated.request, input.request);
    assert_eq!(input.skill_id, None);
    assert_eq!(input.provider_deployment_id, Some(provider));
    assert_eq!(
        input.request,
        json!({"contract":contract,"provider_deployment_id":provider,
                                "input":{"x":17},"idempotency_key":"stable","timeout_ms":300000})
    );
    spec["steps"][3]["input_mapping"]["x"]["step_output"]["step"] = json!("side");
    tx.execute("UPDATE workflows SET spec=$1", &[&spec])
        .await
        .unwrap();
    assert_eq!(
        build(&tx, step, deadline).await.unwrap_err().code,
        "INVALID_ARGUMENT"
    );
    let current = &mut spec["steps"][3];
    current["input_mapping"] = json!({});
    current["contract"]["name"] = json!("automation.execute");
    tx.execute("UPDATE workflows SET spec=$1", &[&spec])
        .await
        .unwrap();
    assert_eq!(
        build(&tx, step, deadline).await.unwrap_err().code,
        "NOT_READY"
    );
    let current = &mut spec["steps"][3];
    current.as_object_mut().unwrap().remove("contract");
    current["skill_id"] = json!(skill);
    tx.execute("UPDATE workflows SET spec=$1", &[&spec])
        .await
        .unwrap();
    let definition =
        json!({"maximum_timeout_ms":90500,"target":{"kind":"capability","contract":contract}});
    tx.execute("INSERT INTO skills VALUES ($1,$2)", &[&skill, &definition])
        .await
        .unwrap();
    let input = build(&tx, step, deadline).await.unwrap();
    assert_eq!(input.skill_id, Some(skill));
    assert_eq!(
        input.request,
        json!({"provider_deployment_id":provider,"input":{},
                                    "idempotency_key":"stable","timeout_ms":90500})
    );

    // A Skill cannot route the long-running capability through the unary Broker.
    tx.execute("UPDATE skills SET definition=jsonb_set(definition,'{target,contract,name}',to_jsonb('automation.execute'::text))",&[]).await.unwrap();
    assert_eq!(
        build(&tx, step, deadline).await.unwrap_err().code,
        "NOT_READY"
    );

    // HTTP operation names are scoped to the HTTP contract.
    let http = json!({"maximum_timeout_ms":90500,"target":{"kind":"http","http_operation_id":"automation.execute","plugin_id":"org.example.echo","http_contract_digest":crate::contracts::digest(b"http")}});
    tx.execute("UPDATE skills SET definition=$1", &[&http])
        .await
        .unwrap();
    assert_eq!(
        build(&tx, step, deadline).await.unwrap().request,
        input.request
    );
}
