//! Result-read and idempotency gates against actual PostgreSQL and the public
//! Router. Calls have explicitly constructed completed metadata; this test does
//! not claim that a provider executed, or that an OCI workload was admitted.
use super::*;
use crate::{
    auth::{Context, Resource},
    policy::{self, Policy},
};
use std::collections::BTreeSet;
use tokio_postgres::Client;

// These three tests validate frozen result authority, not throughput or
// concurrency. Keep their independent KDF/policy-heavy metadata fixtures from
// competing with each other; request deadlines and all production limits stay
// unchanged. Real shared-instance concurrency has its own OCI acceptance.
static RESULT_FIXTURE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn grant(c: &Client, issuer: &Context, subject: Uuid, action: &str, resource: Value) -> Uuid {
    let group = Uuid::new_v4();
    let id = Uuid::new_v4();
    let rules = json!([{"action":action,"resource":resource}]);
    c.execute("INSERT INTO permission_groups(id,owner_id,name,visibility,latest_version,status,created_at,updated_at) VALUES($1,$2,$3,'private',1,'active',now(),now())", &[&group,&issuer.subject,&format!("result gate {group}")]).await.unwrap();
    c.execute("INSERT INTO group_versions(group_id,version,rules,created_by,created_at) VALUES($1,1,$2,$3,now())", &[&group,&rules,&issuer.subject]).await.unwrap();
    c.execute("INSERT INTO grants(id,subject_id,issuer_id,issuer_credential_id,group_id,group_version,scope,status,created_at,updated_at) VALUES($1,$2,$3,$4,$5,1,$6,'active',now(),now())", &[&id,&subject,&issuer.subject,&issuer.credential,&group,&resource]).await.unwrap();
    id
}

struct Completed {
    fixture: Fixture,
    issuer: Context,
    caller: Context,
    target_grant: Uuid,
    blob_grant: Option<Uuid>,
    target_action: &'static str,
    target_resource: Value,
    skill: Uuid,
    call: Uuid,
    request: Value,
    key: String,
    source: Option<Uuid>,
}

impl Completed {
    async fn new(http: bool) -> Option<Self> {
        let mut f = Fixture::new().await?;
        let (deployment, instance) = f.workload().await;
        let source = if http {
            None
        } else {
            Some(
                f.source(b"actual private input for result gate".to_vec())
                    .await,
            )
        };
        let mut c = f.app.db.connect().await.unwrap();
        let issuer = f
            .app
            .auth
            .authenticate(&c, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
            .await
            .unwrap();
        let row=c.query_one("SELECT n.id,n.publisher_id,n.manifest FROM installations n JOIN deployments d ON d.installation_id=n.id WHERE d.id=$1", &[&deployment]).await.unwrap();
        let installation: Uuid = row.get("id");
        let publisher: Uuid = row.get("publisher_id");
        let mut manifest: Value = row.get("manifest");
        let mut raw_contract = None;
        let _http_artifact = if http {
            let (directory, signed, raw) =
                crate::http_contracts::tests::fixture_manifest(publisher);
            manifest = signed;
            raw_contract = Some(raw);
            Some(directory)
        } else {
            None
        };
        let definition = if http {
            manifest["skills"][0].clone()
        } else {
            let cap = &manifest["capabilities"][0];
            json!({"name":"org.xelab.echo-python.result-gate","version":"1.0.0","description":"Fixed result authority fixture","input_schema":cap["input_schema"],"output_schema":cap["output_schema"],"stream_item_schema":null,"target":{"kind":"capability","plugin_id":manifest["plugin_id"],"contract":cap["contract"]},"mapping":"identity","maximum_timeout_ms":10000,"input_blobs":true,"tags":[]})
        };
        manifest["skills"] = json!([definition]);
        crate::artifact::validate_profile(&manifest).unwrap();
        let plugin = manifest["plugin_id"].as_str().unwrap();
        c.execute(
            "UPDATE installations SET plugin_id=$2,manifest=$3,manifest_digest=$4 WHERE id=$1",
            &[
                &installation,
                &plugin,
                &manifest,
                &contracts::digest(&serde_json::to_vec(&manifest).unwrap()),
            ],
        )
        .await
        .unwrap();
        if http {
            crate::http_contracts::store(&c, installation, &manifest, raw_contract.as_deref())
                .await
                .unwrap();
            let approval = json!({"rules":[],"egress":[]});
            c.execute(
                "UPDATE deployments SET approval=$2 WHERE id=$1",
                &[&deployment, &approval],
            )
            .await
            .unwrap();
            crate::host_blobs::approve(
                &c,
                &issuer,
                deployment,
                &manifest,
                &approval,
                issuer.subject,
            )
            .await
            .unwrap();
            crate::plugin_http_gateway::approve(
                &c,
                &issuer,
                deployment,
                &manifest,
                &approval,
                issuer.subject,
            )
            .await
            .unwrap();
        } else {
            let cap = &manifest["capabilities"][0];
            c.execute("INSERT INTO capability_contracts(name,version,schema_digest,spec) VALUES($1,$2,$3,$4)", &[&cap["contract"]["name"].as_str().unwrap(),&cap["contract"]["version"].as_str().unwrap(),&cap["contract"]["schema_digest"].as_str().unwrap(),&cap]).await.unwrap();
            c.execute("INSERT INTO provider_records(instance_id,name,version,availability,features,observed_generation,observed_at) VALUES($1,$2,$3,'ready','[]',7,now())", &[&instance,&cap["contract"]["name"].as_str().unwrap(),&cap["contract"]["version"].as_str().unwrap()]).await.unwrap();
        }
        c.execute(
            "UPDATE deployments SET observed_generation=7 WHERE id=$1",
            &[&deployment],
        )
        .await
        .unwrap();
        c.execute(
            "UPDATE instances SET last_heartbeat_at=now() WHERE id=$1",
            &[&instance],
        )
        .await
        .unwrap();
        let (skill, _) = crate::skills::execute(
            &c,
            &f.app.auth,
            &issuer,
            "registerSkill",
            &std::collections::HashMap::new(),
            &json!({"installation_id":installation,"definition":definition,"enabled":true}),
            &axum::http::HeaderMap::new(),
            "",
        )
        .await
        .unwrap();
        let skill = contracts::id(&skill, "id").unwrap();
        let subject = Uuid::new_v4();
        let password = "result authority Guest password";
        c.execute("INSERT INTO accounts(id,kind,status,display_name,password_hash,created_at,updated_at) VALUES($1,'user','active','Result reader',$2,now(),now())", &[&subject,&auth::password_hash(password).unwrap()]).await.unwrap();
        c.execute(
            "INSERT INTO account_closure(ancestor_id,descendant_id,depth) VALUES($1,$1,0)",
            &[&subject],
        )
        .await
        .unwrap();
        c.execute("INSERT INTO quotas(account_id,spec,daily_bucket,created_at,updated_at) VALUES($1,$2,current_date,now(),now())", &[&subject,&f.app.config.value["quota_defaults"]]).await.unwrap();
        let issued = f
            .app
            .auth
            .login(&c, subject, password.into(), issuer.peer, Uuid::new_v4())
            .await
            .unwrap();
        f.token = issued["access_token"].as_str().unwrap().to_owned();
        let caller = f
            .app
            .auth
            .authenticate(&c, &f.token, issuer.peer, Uuid::new_v4())
            .await
            .unwrap();
        let (target_action, target_resource) = if http {
            (
                "plugin.http.invoke",
                json!({"kind":"plugin","match":"ids","ids":[plugin]}),
            )
        } else {
            let reference = &definition["target"]["contract"];
            (
                "capability.invoke",
                json!({"kind":"capability","match":"ids","ids":[format!("{}@{}",reference["name"].as_str().unwrap(),reference["version"].as_str().unwrap())]}),
            )
        };
        let target_grant =
            grant(&c, &issuer, subject, target_action, target_resource.clone()).await;
        grant(
            &c,
            &issuer,
            subject,
            "skill.invoke",
            json!({"kind":"skill","match":"ids","ids":[skill]}),
        )
        .await;
        grant(
            &c,
            &issuer,
            subject,
            "capability.call.read",
            json!({"kind":"call","match":"owned_by","owner_id":subject}),
        )
        .await;
        let blob_grant = if let Some(source) = source {
            Some(
                grant(
                    &c,
                    &issuer,
                    subject,
                    "blob.read",
                    json!({"kind":"blob","match":"ids","ids":[source]}),
                )
                .await,
            )
        } else {
            None
        };
        let request = json!({"input":if let Some(source)=source{json!({"source":source})}else{json!({"message":"private result"})},"timeout_ms":5000,"provider_deployment_id":deployment});
        let key = "same completed business request".to_owned();
        let (call, contract, fingerprint) = if http {
            let call = Uuid::new_v4();
            let registry = crate::http_contracts::load(&c, installation).await.unwrap();
            let operation = registry.bind_skill(&definition).unwrap();
            let binding = json!({"installation_id":installation,"deployment_id":deployment,"generation":7,"http_contract_digest":operation.contract_digest,"operation_id":operation.id,"method":operation.method.as_str(),"path":operation.path,"required_action":operation.route.action});
            let contract = json!({"name":definition["name"],"version":definition["version"],"schema_digest":crate::skills::definition_digest(&definition).unwrap()});
            let tx = c.transaction().await.unwrap();
            crate::db::call_lock(&tx).await.unwrap();
            tx.execute("INSERT INTO calls(id,owner_id,credential_id,provider_instance_id,skill_id,skill_definition_digest,contract,input_digest,idempotency_key,state,deadline_at,created_at,http_binding) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'running',now()+interval '2 minutes',now(),$10)", &[&call,&subject,&caller.credential,&instance,&skill,&crate::skills::definition_digest(&definition).unwrap(),&contract,&contracts::digest(&serde_json::to_vec(&request["input"]).unwrap()),&key,&binding]).await.unwrap();
            let resource = Resource::new("plugin", plugin, Some(issuer.subject));
            let paths = Policy::for_context(&tx, &caller)
                .with_resource_owner(&resource)
                .credential(caller.credential, 0)
                .await
                .unwrap();
            let mut charged = BTreeSet::new();
            for (action, resource) in [
                (target_action, resource),
                (
                    "skill.invoke",
                    Resource::new("skill", skill, Some(issuer.subject)),
                ),
            ] {
                charged.extend(
                    crate::identity::call_sources_from_paths(
                        &tx,
                        &caller,
                        action,
                        &resource,
                        paths.clone(),
                    )
                    .await
                    .unwrap(),
                );
            }
            for credential in charged {
                tx.execute(
                    "INSERT INTO call_credential_charges(call_id,credential_id) VALUES($1,$2)",
                    &[&call, &credential],
                )
                .await
                .unwrap();
            }
            crate::host_blobs::grant_skill_call(
                &tx,
                &f.app,
                &caller,
                call,
                deployment,
                &manifest,
                &definition,
                &request["input"],
                false,
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
            let headers = std::collections::BTreeMap::from([(
                "content-type".to_owned(),
                b"application/json".to_vec(),
            )]);
            let fingerprint=crate::skills::definition_digest(&json!({"installation":installation,"deployment":deployment,"operation":operation.id,"digest":operation.contract_digest,"input":request["input"],"headers":headers,"skill_input":request})).unwrap();
            (call, contract, fingerprint)
        } else {
            let (call, _) = f
                .skill_delegation(deployment, instance, source.unwrap(), Some(skill), true)
                .await;
            (
                call,
                definition["target"]["contract"].clone(),
                contracts::digest(&serde_json::to_vec(&request).unwrap()),
            )
        };
        let result = json!({"call_id":call,"contract":contract,"provider_instance_id":instance,"output":{"message":"secret completed result"},"completed_at":chrono::Utc::now(),"skill_id":skill,"skill_definition_digest":crate::skills::definition_digest(&definition).unwrap()});
        contracts::validate("CallResult", &result).unwrap();
        c.execute(
            "UPDATE calls SET state='succeeded',result=$2,idempotency_key=$3,input_digest=$4,completed_at=now() WHERE id=$1",
            &[&call, &result, &key, &contracts::digest(&serde_json::to_vec(&request["input"]).unwrap())],
        )
        .await
        .unwrap();
        c.execute("UPDATE execution_delegations SET status='revoked' WHERE id=(SELECT delegation_id FROM calls WHERE id=$1)", &[&call]).await.unwrap();
        c.execute(
            "UPDATE rpc_delegations SET status='revoked' WHERE call_id=$1",
            &[&call],
        )
        .await
        .unwrap();
        c.execute("INSERT INTO idempotency_records(subject_id,credential_id,method,path,key,fingerprint,resource_id,expires_at,created_at) VALUES($1,$2,'POST',$3,$4,$5,$6,now()+interval '24 hours',now())", &[&subject,&caller.credential,&format!("/api/platform/v1/skills/{skill}/calls"),&key,&fingerprint,&call.to_string()]).await.unwrap();
        Some(Self {
            fixture: f,
            issuer,
            caller,
            target_grant,
            blob_grant,
            target_action,
            target_resource,
            skill,
            call,
            request,
            key,
            source,
        })
    }

    async fn observe_provider(&self) {
        // Explicit runtime metadata is this component's prerequisite. There is
        // no provider process sending heartbeats; each request must reach the
        // current authorization gate rather than an expired readiness fixture.
        // No phase, generation, approval or authority is changed here.
        let c = self.fixture.app.db.connect().await.unwrap();
        c.execute("UPDATE instances SET last_heartbeat_at=now() WHERE id=(SELECT provider_instance_id FROM calls WHERE id=$1)", &[&self.call]).await.unwrap();
        c.execute("UPDATE provider_records SET observed_at=now() WHERE instance_id=(SELECT provider_instance_id FROM calls WHERE id=$1)", &[&self.call]).await.unwrap();
    }

    async fn get(&self) -> (u16, Value) {
        self.observe_provider().await;
        self.fixture
            .call(
                "getCall",
                &[("call_id", self.call)],
                Value::Null,
                None,
                "read-existing-result",
                None,
            )
            .await
    }
    async fn replay(&self) -> (u16, Value) {
        self.observe_provider().await;
        self.fixture
            .call(
                "invokeSkill",
                &[("skill_id", self.skill)],
                self.request.clone(),
                None,
                &self.key,
                None,
            )
            .await
    }
    async fn counters(&self) -> Value {
        let c = self.fixture.app.db.connect().await.unwrap();
        let row=c.query_one("SELECT (SELECT count(*) FROM calls WHERE owner_id=$1) calls,(SELECT count(*) FROM reservations WHERE account_id=$1) reservations,(SELECT count(*) FROM call_credential_charges q JOIN calls c ON c.id=q.call_id WHERE c.owner_id=$1) charges,q.daily_calls_used,q.concurrent_calls_used FROM quotas q WHERE q.account_id=$1", &[&self.caller.subject]).await.unwrap();
        json!({"calls":row.get::<_,i64>("calls"),"reservations":row.get::<_,i64>("reservations"),"charges":row.get::<_,i64>("charges"),"daily":row.get::<_,i64>("daily_calls_used"),"concurrent":row.get::<_,i64>("concurrent_calls_used")})
    }
}

async fn original_business_proof_cannot_be_replaced(http: bool) {
    let _fixture_isolation = RESULT_FIXTURE.lock().await;
    let Some(f) = Completed::new(http).await else {
        return;
    };
    let initial = f.get().await;
    assert_eq!(initial.0, 200, "{}", initial.1);
    assert_eq!(
        initial.1["result"]["output"]["message"],
        "secret completed result"
    );
    let replay = f.replay().await;
    assert_eq!(replay.0, 200, "{}", replay.1);
    let before = f.counters().await;
    let c = f.fixture.app.db.connect().await.unwrap();
    if http {
        let deployment = contracts::id(&f.request, "provider_deployment_id").unwrap();
        c.execute(
            "DELETE FROM plugin_http_approvals WHERE deployment_id=$1",
            &[&deployment],
        )
        .await
        .unwrap();
        for denied in [f.get().await, f.replay().await] {
            assert_eq!(denied.0, 403, "{}", denied.1);
            assert!(!denied.1.to_string().contains("secret completed result"));
        }
        assert_eq!(
            before,
            f.counters().await,
            "withdrawing route approval cannot expose or recharge a completed result"
        );
        let row=c.query_one("SELECT n.manifest,d.approval,d.owner_id FROM deployments d JOIN installations n ON n.id=d.installation_id WHERE d.id=$1", &[&deployment]).await.unwrap();
        crate::plugin_http_gateway::approve(
            &c,
            &f.issuer,
            deployment,
            &row.get("manifest"),
            &row.get("approval"),
            row.get("owner_id"),
        )
        .await
        .unwrap();
        assert_eq!(f.get().await.0, 200);
    }
    c.execute(
        "UPDATE grants SET status='revoked',revision=revision+1,updated_at=now() WHERE id=$1",
        &[&f.target_grant],
    )
    .await
    .unwrap();
    grant(
        &c,
        &f.issuer,
        f.caller.subject,
        f.target_action,
        f.target_resource.clone(),
    )
    .await;
    let resource = if http {
        Resource::new(
            "plugin",
            f.target_resource["ids"][0].as_str().unwrap(),
            Some(f.issuer.subject),
        )
    } else {
        Resource::new(
            "capability",
            f.target_resource["ids"][0].as_str().unwrap(),
            None,
        )
    };
    let paths = Policy::for_context(&c, &f.caller)
        .with_resource_owner(&resource)
        .credential(f.caller.credential, 0)
        .await
        .unwrap();
    assert!(
        auth::rules_allow(
            &policy::rules(&paths),
            f.target_action,
            &resource,
            f.caller.peer
        ),
        "the unrelated new grant is actually valid for future work"
    );
    for denied in [f.get().await, f.replay().await] {
        assert_eq!(denied.0, 403, "{}", denied.1);
        assert_eq!(denied.1["error"]["code"], "PERMISSION_DENIED");
        assert!(!denied.1.to_string().contains("secret completed result"));
    }
    assert_eq!(
        before,
        f.counters().await,
        "result denial must not admit or charge new work"
    );
}

#[tokio::test]
async fn actual_postgres_rpc_get_and_replay_preserve_original_charged_authority() {
    original_business_proof_cannot_be_replaced(false).await;
}

#[tokio::test]
async fn actual_postgres_http_get_and_replay_preserve_original_charged_authority() {
    original_business_proof_cannot_be_replaced(true).await;
}

#[tokio::test]
async fn actual_postgres_input_blob_revocation_blocks_stored_results_and_rolls_back_new_admission()
{
    let _fixture_isolation = RESULT_FIXTURE.lock().await;
    let Some(f) = Completed::new(false).await else {
        return;
    };
    assert_eq!(f.get().await.0, 200);
    assert_eq!(f.replay().await.0, 200);
    let before = f.counters().await;
    let c = f.fixture.app.db.connect().await.unwrap();
    c.execute(
        "UPDATE grants SET status='revoked',revision=revision+1,updated_at=now() WHERE id=$1",
        &[&f.blob_grant.unwrap()],
    )
    .await
    .unwrap();
    auth::authorize(
        &c,
        &f.caller,
        "skill.invoke",
        &Resource::new("skill", f.skill, Some(f.issuer.subject)),
    )
    .await
    .unwrap();
    let cap = Resource::new(
        "capability",
        f.target_resource["ids"][0].as_str().unwrap(),
        None,
    );
    auth::authorize(&c, &f.caller, "capability.invoke", &cap)
        .await
        .unwrap();
    assert!(auth::authorize(
        &c,
        &f.caller,
        "blob.read",
        &Resource::new("blob", f.source.unwrap(), Some(f.issuer.subject))
    )
    .await
    .is_err());
    for denied in [f.get().await, f.replay().await] {
        assert_eq!(denied.0, 403, "{}", denied.1);
        assert!(!denied.1.to_string().contains("secret completed result"));
    }
    // This test has no running provider process. Refresh its explicit runtime
    // observation immediately before admission so the negative path reaches
    // Blob authorization rather than an unrelated expired readiness fixture.
    f.observe_provider().await;
    let denied = f
        .fixture
        .call(
            "invokeSkill",
            &[("skill_id", f.skill)],
            f.request.clone(),
            None,
            "fresh-after-input-revocation",
            None,
        )
        .await;
    assert_eq!(denied.0, 403, "{}", denied.1);
    assert!(!denied.1.to_string().contains("secret completed result"));
    assert_eq!(
        before,
        f.counters().await,
        "failed Blob admission must roll back Call, reservation, source charges and quota"
    );
    // A new Blob grant can authorize future work, but cannot revive data from a
    // call whose admission-time Blob proof has been withdrawn.
    grant(
        &c,
        &f.issuer,
        f.caller.subject,
        "blob.read",
        json!({"kind":"blob","match":"ids","ids":[f.source.unwrap()]}),
    )
    .await;
    assert_eq!(f.get().await.0, 403);
    assert_eq!(f.replay().await.0, 403);
    assert_eq!(before, f.counters().await);
}
