//! Least-privilege predicates over actual Skill/Plugin/Blob owners. Workload
//! and ancestry metadata are explicit fixtures; transport requests use mTLS.
use super::*;

async fn scoped_proxy(f: &Fixture, deployment: Uuid, rules: Value, token: &str) {
    let mut c = f.app.db.connect().await.unwrap();
    let tx = c.transaction().await.unwrap();
    let ctx = f
        .app
        .auth
        .authenticate(&tx, token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    let installation: Uuid = tx
        .query_one(
            "SELECT installation_id FROM deployments WHERE id=$1",
            &[&deployment],
        )
        .await
        .unwrap()
        .get(0);
    let mut manifest: Value = tx
        .query_one(
            "SELECT manifest FROM installations WHERE id=$1",
            &[&installation],
        )
        .await
        .unwrap()
        .get(0);
    manifest["requested_rules"] = json!(rules
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| json!({"required":false,"rule":rule}))
        .collect::<Vec<_>>());
    crate::artifact::validate_profile(&manifest).unwrap();
    let approval = json!({"rules":rules,"egress":[]});
    tx.execute(
        "UPDATE installations SET manifest=$2 WHERE id=$1",
        &[&installation, &manifest],
    )
    .await
    .unwrap();
    crate::host_blobs::approve(&tx, &ctx, deployment, &manifest, &approval, ctx.subject)
        .await
        .unwrap();
    tx.execute(
        "UPDATE deployments SET approval=$2 WHERE id=$1",
        &[&deployment, &approval],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

async fn clone_skill(f: &Fixture, original: Uuid, name_suffix: &str, owner: Uuid) -> Uuid {
    let c = f.app.db.connect().await.unwrap();
    let row = c
        .query_one(
            "SELECT installation_id,definition FROM skills WHERE id=$1",
            &[&original],
        )
        .await
        .unwrap();
    let installation: Uuid = row.get(0);
    let mut definition: Value = row.get(1);
    definition["name"] = json!(format!(
        "{}.{name_suffix}",
        definition["name"].as_str().unwrap()
    ));
    let mut manifest: Value = c
        .query_one(
            "SELECT manifest FROM installations WHERE id=$1",
            &[&installation],
        )
        .await
        .unwrap()
        .get(0);
    manifest["skills"]
        .as_array_mut()
        .unwrap()
        .push(definition.clone());
    c.execute(
        "UPDATE installations SET manifest=$2 WHERE id=$1",
        &[&installation, &manifest],
    )
    .await
    .unwrap();
    let id = Uuid::new_v4();
    c.execute("INSERT INTO skills(id,name,version,installation_id,created_by,definition,definition_digest,enabled,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,true,now(),now())",&[&id,&definition["name"].as_str().unwrap(),&definition["version"].as_str().unwrap(),&installation,&owner,&definition,&crate::skills::definition_digest(&definition).unwrap()]).await.unwrap();
    id
}

#[tokio::test]
async fn host_mtls_ids_approval_and_owned_by_caller_intersect_without_widening() {
    let Some(mut f) = Fixture::new().await else {
        return;
    };
    let root_token = f.token.clone();
    let (consumer, consumer_instance) = f.workload().await;
    let (provider, provider_instance) = f.workload().await;
    let (target, definition) = skill(&f, provider).await;
    let c = f.app.db.connect().await.unwrap();
    let owner: Uuid = c
        .query_one("SELECT created_by FROM skills WHERE id=$1", &[&target])
        .await
        .unwrap()
        .get(0);
    let foreign_owner = Uuid::new_v4();
    c.execute("INSERT INTO accounts(id,kind,status,display_name,created_at,updated_at) VALUES($1,'user','active','foreign fixture owner',now(),now())",&[&foreign_owner]).await.unwrap();
    c.execute(
        "INSERT INTO account_closure(ancestor_id,descendant_id,depth) VALUES($1,$1,0)",
        &[&foreign_owner],
    )
    .await
    .unwrap();
    let foreign = clone_skill(&f, target, "foreign", foreign_owner).await;
    let plugin = definition["target"]["plugin_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let rules = json!([
        {"action":"blob.read","resource":{"kind":"blob","match":"all"}},
        {"action":"blob.create","resource":{"kind":"blob","match":"all"}},
        {"action":"capability.invoke","resource":{"kind":"capability","match":"all"}},
        {"action":"skill.read","resource":{"kind":"skill","match":"ids","ids":[target,foreign]}},
        {"action":"skill.invoke","resource":{"kind":"skill","match":"ids","ids":[target,foreign]}},
        {"action":"plugin.read","resource":{"kind":"plugin","match":"ids","ids":[plugin]}},
        {"action":"event.publish","resource":{"kind":"event","match":"owned_by","owner_id":owner}}
    ]);
    scoped_proxy(&f, consumer, rules.clone(), &root_token).await;
    let selected = json!([
        {"action":"blob.read","resource":{"kind":"blob","match":"owned_by","owner_id":owner}},
        {"action":"blob.create","resource":{"kind":"blob","match":"owned_by","owner_id":owner}},
        {"action":"capability.invoke","resource":{"kind":"capability","match":"all"}},
        {"action":"skill.read","resource":{"kind":"skill","match":"owned_by","owner_id":owner}},
        {"action":"skill.invoke","resource":{"kind":"skill","match":"owned_by","owner_id":owner}},
        {"action":"plugin.read","resource":{"kind":"plugin","match":"owned_by","owner_id":owner}},
        {"action":"event.publish","resource":{"kind":"event","match":"owned_by","owner_id":owner}}
    ]);
    let expiry = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let issued=f.call("createApiKey",&[("account_id",owner)],json!({"name":"owned resource Host caller","inheritance":{"mode":"selected","rules":selected},"conditions":{"expires_at":expiry}}),None,"layered-owned-key",Some(&root_token)).await;
    assert_eq!(issued.0, 201, "{}", issued.1);
    f.token = issued.1["secret"].as_str().unwrap().to_owned();
    let source = f.source(b"owned input source".to_vec()).await;
    let (parent, context) = f.delegation(consumer, consumer_instance, source).await;
    let (echo, downstream) = echo(&f, provider_instance).await;
    let host = tokio::spawn(crate::host::serve(f.app.clone()));
    let mut client = f
        .client(f.client_identity(&format!(
            "spiffe://xelab.local/instance/{consumer_instance}/generation/7"
        )))
        .await;
    let resolved = client
        .resolve_skill(rpc::ResolveSkillRequest {
            context: Some(context.clone()),
            skill_id: target.to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(
        resolved.ready,
        "concrete owned Skill UUID must satisfy both IDs and owned_by"
    );
    assert!(
        client
            .check_skill_access(rpc::CheckSkillAccessRequest {
                context: Some(context.clone()),
                skill_id: target.to_string(),
                expected_definition_digest: resolved.definition_digest
            })
            .await
            .unwrap()
            .into_inner()
            .ready
    );
    let input = serde_json::to_vec(&json!({"source":source})).unwrap();
    let result = client
        .invoke_skill(rpc::SkillInvokeRequest {
            call_id: None,
            context: Some(context.clone()),
            skill_id: target.to_string(),
            input_digest: contracts::digest(&input),
            input_json: input,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        serde_json::from_slice::<Value>(&result.output_json).unwrap(),
        json!({"message":"proxied"})
    );
    assert_eq!(echo.calls.load(Ordering::SeqCst), 1);
    assert!(
        client
            .resolve_skill(rpc::ResolveSkillRequest {
                context: Some(context.clone()),
                skill_id: foreign.to_string()
            })
            .await
            .is_err(),
        "an approved UUID with a different actual owner remains denied"
    );
    // Check actual plugin owner directly through the same authenticated actor.
    let mut conn = f.app.db.connect().await.unwrap();
    let tx = conn.transaction().await.unwrap();
    host_calls::authority(
        &tx,
        &f.app,
        Workload {
            instance: consumer_instance,
            generation: 7,
        },
        &context,
        "plugin.read",
        &auth::Resource::new("plugin", plugin.clone(), Some(owner)),
    )
    .await
    .unwrap();
    assert!(host_calls::authority(
        &tx,
        &f.app,
        Workload {
            instance: consumer_instance,
            generation: 7
        },
        &context,
        "plugin.read",
        &auth::Resource::new("plugin", plugin.clone(), Some(foreign_owner))
    )
    .await
    .is_err());
    assert!(
        host_calls::authority(
            &tx,
            &f.app,
            Workload {
                instance: consumer_instance,
                generation: 7
            },
            &context,
            "event.publish",
            &auth::Resource::new("event", "org.xelab.namespace", None)
        )
        .await
        .is_err(),
        "namespace events have no owner; owned_by must not manufacture one"
    );
    tx.rollback().await.unwrap();
    let late = clone_skill(&f, target, "late", owner).await;
    let mut expanded = rules;
    for rule in expanded
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .filter(|rule| rule["resource"]["kind"] == "skill")
    {
        rule["resource"]["ids"]
            .as_array_mut()
            .unwrap()
            .push(json!(late));
    }
    scoped_proxy(&f, consumer, expanded, &root_token).await;
    let root_credential: Uuid = f
        .app
        .auth
        .authenticate(
            &c,
            &root_token,
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap()
        .credential;
    c.execute("INSERT INTO grants(id,subject_id,issuer_id,issuer_credential_id,role_id,scope,status,created_at,updated_at) VALUES($1,$2,$2,$3,'Owner',$4,'active',now(),now())",&[&Uuid::new_v4(),&owner,&root_credential,&json!({"kind":"skill","match":"ids","ids":[foreign,late]})]).await.unwrap();
    assert!(
        client
            .resolve_skill(rpc::ResolveSkillRequest {
                context: Some(context.clone()),
                skill_id: late.to_string()
            })
            .await
            .is_err(),
        "new approval and source grant cannot widen the original frozen ID ceiling"
    );
    assert!(
        client
            .resolve_skill(rpc::ResolveSkillRequest {
                context: Some(context),
                skill_id: foreign.to_string()
            })
            .await
            .is_err(),
        "new grant cannot widen the selected caller's owned_by ceiling"
    );
    assert_eq!(
        c.query_one("SELECT state FROM calls WHERE id=$1", &[&parent])
            .await
            .unwrap()
            .get::<_, String>(0),
        "running"
    );
    host.abort();
    downstream.abort();
}

#[tokio::test]
async fn host_child_blob_rpc_obeys_current_ancestor_ceiling() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let root = f.token.clone();
    let (ancestor, ancestor_instance) = f.workload().await;
    let (middle, middle_instance) = f.workload().await;
    proxy(&f, middle, proxy_rules()).await;
    let source = f.source(b"child Blob source bytes".to_vec()).await;
    let other = f.source(b"unrelated Blob".to_vec()).await;
    let rules = |blob| {
        json!([
            {"action":"blob.read","resource":{"kind":"blob","match":"ids","ids":[blob]}},
            {"action":"blob.create","resource":{"kind":"blob","match":"all"}},
            {"action":"capability.invoke","resource":{"kind":"capability","match":"all"}}
        ])
    };
    scoped_proxy(&f, ancestor, rules(source), &root).await;
    let (parent, parent_context) = f.delegation(ancestor, ancestor_instance, source).await;
    let (child, context) = f.delegation(middle, middle_instance, source).await;
    synthetic_ancestry(&f, parent, ancestor_instance, &parent_context, child).await;
    let host = tokio::spawn(crate::host::serve(f.app.clone()));
    let mut client = f
        .client(f.client_identity(&format!(
            "spiffe://xelab.local/instance/{middle_instance}/generation/7"
        )))
        .await;
    let request = rpc::ReadBlobRequest {
        context: Some(context.clone()),
        blob_id: source.to_string(),
    };
    let mut stream = client
        .read_blob(request.clone())
        .await
        .unwrap()
        .into_inner();
    let mut bytes = Vec::new();
    while let Some(frame) = stream.message().await.unwrap() {
        bytes.extend(frame.data)
    }
    assert_eq!(bytes, b"child Blob source bytes");
    scoped_proxy(&f, ancestor, rules(other), &root).await;
    let failure = match client.read_blob(request).await {
        Err(status) => status,
        Ok(response) => response.into_inner().message().await.unwrap_err(),
    };
    assert_eq!(
        failure.code(),
        tonic::Code::PermissionDenied,
        "child's broad Blob approval/input allowlist cannot bypass a narrower ancestor"
    );
    let c = f.app.db.connect().await.unwrap();
    for call in [parent, child] {
        assert_eq!(
            c.query_one("SELECT state FROM calls WHERE id=$1", &[&call])
                .await
                .unwrap()
                .get::<_, String>(0),
            "running"
        )
    }
    host.abort();
}

#[tokio::test]
async fn host_queued_child_result_cannot_bypass_ancestor_ceiling_narrowed_after_admission() {
    use futures_util::StreamExt;
    let Some(f) = Fixture::new().await else {
        return;
    };
    let root = f.token.clone();
    let (ancestor, ancestor_instance) = f.workload().await;
    let (middle, middle_instance) = f.workload().await;
    let (provider, provider_instance) = f.workload().await;
    let contract = capability_target(&f, provider, provider_instance).await;
    proxy(&f, middle, proxy_rules()).await;
    let rules = |target: &str| {
        json!([
            {"action":"blob.read","resource":{"kind":"blob","match":"all"}},
            {"action":"blob.create","resource":{"kind":"blob","match":"all"}},
            {"action":"capability.discover","resource":{"kind":"capability","match":"all"}},
            {"action":"capability.invoke","resource":{"kind":"capability","match":"ids","ids":[target]}}
        ])
    };
    let mut initial_rules = rules(&format!("{}@{}", contract.name, contract.version));
    initial_rules[3]["resource"]["ids"]
        .as_array_mut()
        .unwrap()
        .push(json!("org.xelab.echo@1.0.0"));
    scoped_proxy(&f, ancestor, initial_rules, &root).await;
    dependency(&f, middle, &contract).await;
    let source = f.source(b"ancestor queued result".to_vec()).await;
    let (parent, parent_context) = f.delegation(ancestor, ancestor_instance, source).await;
    let (child, context) = f.delegation(middle, middle_instance, source).await;
    synthetic_ancestry(&f, parent, ancestor_instance, &parent_context, child).await;
    let (_, downstream) = echo(&f, provider_instance).await;
    let c = f.app.db.connect().await.unwrap();
    let actual_ctx = f
        .app
        .auth
        .authenticate(&c, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    let binding=crate::dependencies::resolve_host(&c,&actual_ctx,middle,child,&child.to_string(),&json!({"name":contract.name,"version":contract.version,"schema_digest":contract.schema_digest})).await.unwrap();
    let request = invocation(
        &context,
        &contract,
        binding["binding_id"].as_str().unwrap(),
        source,
    );
    let grandchild: Uuid = request.call_id.parse().unwrap();
    let mut stream = host_calls::invoke_stream(
        f.app.clone(),
        Workload {
            instance: middle_instance,
            generation: 7,
        },
        request,
    )
    .await
    .unwrap();
    let mut finished = false;
    for _ in 0..500 {
        if let Some(row) = c
            .query_opt("SELECT state FROM calls WHERE id=$1", &[&grandchild])
            .await
            .unwrap()
        {
            if row.get::<_, String>(0) == "succeeded" {
                finished = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        finished,
        "downstream must complete before the queued-result policy test"
    );
    scoped_proxy(&f, ancestor, rules("org.xelab.echo@1.0.0"), &root).await;
    let frame = stream.next().await.unwrap().unwrap_err();
    assert_eq!(
        frame.code(),
        tonic::Code::PermissionDenied,
        "buffered successful result must honor the ancestor's narrowed current predicate"
    );
    assert!(stream.next().await.is_none());
    for call in [parent, child] {
        assert_eq!(
            c.query_one("SELECT state FROM calls WHERE id=$1", &[&call])
                .await
                .unwrap()
                .get::<_, String>(0),
            "running"
        )
    }
    downstream.abort();
}
