use crate::{
    api::App,
    auth::{account_chain, authorize, Resource},
    contracts::{digest, id, strict_json, string, unsigned, validate, validate_business},
    controller, db,
    error::{Error, Result},
    rpc::{self, capability_service_client::CapabilityServiceClient},
    tls,
};
use serde_json::{json, Value};
use std::{net::IpAddr, time::Duration};
use uuid::Uuid;

pub async fn invoke(
    app: &App,
    token: &str,
    peer: IpAddr,
    request_id: Uuid,
    input: &Value,
    key: &str,
) -> Result<Value> {
    invoke_bounded(app, token, peer, request_id, input, key, None, None).await
}

/// Skill identity comes from the path, never from caller-supplied route context.
#[allow(clippy::too_many_arguments)]
pub async fn invoke_skill(
    app: &App,
    token: &str,
    peer: IpAddr,
    request_id: Uuid,
    skill: Uuid,
    input: &Value,
    key: &str,
) -> Result<Value> {
    invoke_bounded(app, token, peer, request_id, input, key, Some(skill), None).await
}

/// Streaming shares the exact ordinary-call admission, journal and finalizer.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream_skill(
    app: &App,
    token: &str,
    peer: IpAddr,
    request_id: Uuid,
    skill: Uuid,
    input: &Value,
    key: &str,
    output: &crate::skill_streams::Output,
) -> Result<Value> {
    invoke_bounded(
        app,
        token,
        peer,
        request_id,
        input,
        key,
        Some(skill),
        Some(output),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn invoke_bounded(
    app: &App,
    token: &str,
    peer: IpAddr,
    request_id: Uuid,
    input: &Value,
    key: &str,
    skill: Option<Uuid>,
    stream: Option<&crate::skill_streams::Output>,
) -> Result<Value> {
    let started = tokio::time::Instant::now();
    let wall_started = chrono::Utc::now();
    let timeout = input
        .get("timeout_ms")
        .map(unsigned)
        .transpose()?
        .unwrap_or(if skill.is_some() { 120000 } else { 30000 })
        .min(300000);
    let expired = || {
        Error::new(
            "DEADLINE_EXCEEDED",
            "Call deadline elapsed; inspect any admitted call before retrying",
            axum::http::StatusCode::GATEWAY_TIMEOUT,
        )
    };
    let invocation = async {
        let mut effective = timeout;
        if let Some(skill) = skill {
            // This read can only narrow the time budget. It conveys no authority;
            // full signed binding and current authorization are checked inside
            // admission. Read before waiting for the shared business call lock.
            let c = app.db.connect().await?;
            if let Some(row) = c
                .query_opt(
                    "SELECT definition->'maximum_timeout_ms' AS maximum FROM skills WHERE id=$1",
                    &[&skill],
                )
                .await?
            {
                let maximum: Option<Value> = row.get("maximum");
                if let Some(maximum) = maximum.and_then(|v| unsigned(&v).ok()) {
                    effective = effective.min(maximum);
                }
            }
        }
        tokio::time::timeout_at(
            started + Duration::from_millis(effective),
            invoke_inner(
                app,
                token,
                peer,
                request_id,
                input,
                key,
                skill,
                stream,
                wall_started,
            ),
        )
        .await
        .unwrap_or_else(|_| Err(expired()))
    };
    // Also bound metadata lookup itself by the caller/default outer budget.
    // Dropping admission rolls back; admitted work retains its durable slot.
    tokio::time::timeout_at(started + Duration::from_millis(timeout), invocation)
        .await
        .unwrap_or_else(|_| Err(expired()))
}

#[allow(clippy::too_many_arguments)]
async fn invoke_inner(
    app: &App,
    token: &str,
    peer: IpAddr,
    request_id: Uuid,
    input: &Value,
    key: &str,
    skill_id: Option<Uuid>,
    stream: Option<&crate::skill_streams::Output>,
    started: chrono::DateTime<chrono::Utc>,
) -> Result<Value> {
    validate(
        if skill_id.is_some() {
            "SkillInvokeRequest"
        } else {
            "InvokeRequest"
        },
        input,
    )?;
    // HTTP replay scope and the plugin's stable business key are distinct.
    let business_key = input["idempotency_key"].as_str().unwrap_or(key);
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::call_lock(&tx).await?;
    let ctx = app.auth.authenticate(&tx, token, peer, request_id).await?;
    let skill = if let Some(id) = skill_id {
        match crate::skills::resolve_for_invoke(&tx, &ctx, id).await {
            Ok(skill) => Some(skill),
            Err(error) => {
                db::audit(
                    &tx,
                    ctx.subject,
                    Some(ctx.credential),
                    request_id,
                    "skill.invoke",
                    &Resource::new("skill", id, None).json(),
                    "denied",
                    error.code,
                )
                .await?;
                tx.commit().await?;
                return Err(error);
            }
        }
    } else {
        None
    };
    if stream.is_some()
        && skill
            .as_ref()
            .is_none_or(|skill| skill.definition["stream_item_schema"].is_null())
    {
        return Err(Error::invalid(
            "Skill does not declare a streaming item schema",
        ));
    }
    let adapted;
    let original = input;
    let input = if let Some(skill) = &skill {
        let maximum = unsigned(&skill.definition["maximum_timeout_ms"])?;
        let timeout = input
            .get("timeout_ms")
            .map(unsigned)
            .transpose()?
            .unwrap_or(120000.min(maximum));
        if timeout > maximum {
            return Err(Error::invalid(
                "Requested deadline exceeds the fixed Skill maximum",
            ));
        }
        validate_business(&skill.definition["input_schema"], &input["input"])?;
        adapted = json!({"contract":skill.definition["target"]["contract"],"input":input["input"],"timeout_ms":timeout});
        &adapted
    } else {
        input
    };
    let contract = &input["contract"];
    let name = string(contract, "name")?;
    let version = string(contract, "version")?;
    let schema_digest = string(contract, "schema_digest")?;
    let authorization = authorize(
        &tx,
        &ctx,
        "capability.invoke",
        &Resource::new("capability", format!("{name}@{version}"), None),
    )
    .await;
    if let Err(e) = authorization {
        db::audit(
            &tx,
            ctx.subject,
            Some(ctx.credential),
            request_id,
            "capability.invoke",
            &Resource::new("capability", format!("{name}@{version}"), None).json(),
            "denied",
            e.code,
        )
        .await?;
        tx.commit().await?;
        return Err(e);
    }
    let fingerprint = digest(&serde_json::to_vec(original)?);
    let path = skill_id.map_or_else(
        || "/api/platform/v1/calls".to_owned(),
        |id| {
            format!(
                "/api/platform/v1/skills/{id}/calls{}",
                if stream.is_some() { ":stream" } else { "" }
            )
        },
    );
    let lock = format!("call:{}:{}:{path}:{key}", ctx.subject, ctx.credential);
    tx.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        &[&lock],
    )
    .await?;
    if let Some(r)=tx.query_opt("SELECT * FROM idempotency_records WHERE subject_id=$1 AND credential_id=$2 AND method='POST' AND path=$4 AND key=$3 AND expires_at>now() FOR UPDATE",&[&ctx.subject,&ctx.credential,&key,&path]).await?{
        if r.get::<_,String>("fingerprint")!=fingerprint{return Err(Error::new("IDEMPOTENCY_CONFLICT","The key was already used for different input",axum::http::StatusCode::CONFLICT));}
        let call_id=r.get::<_,Option<String>>("resource_id").and_then(|s|s.parse::<Uuid>().ok()).ok_or_else(||Error::conflict("Missing prior call"))?;
        if stream.is_some() { return Err(Error::conflict("Generation streams cannot be replayed; inspect the admitted Call").with_resource(call_id)); }
        let row=tx.query_one("SELECT state,result FROM calls WHERE id=$1",&[&call_id]).await?;
        if row.get::<_,String>("state")=="succeeded"{let value = row.get::<_,Option<Value>>("result").ok_or_else(||Error::conflict("Call result is unavailable"))?; return skill_result(skill.as_ref(), value);}
        return Err(Error::new("OUTCOME_UNKNOWN","Prior call is not a confirmed success; query its call ID",axum::http::StatusCode::CONFLICT).with_resource(call_id));
    }
    let cap = tx
        .query_opt(
            "SELECT spec,schema_digest FROM capability_contracts WHERE name=$1 AND version=$2",
            &[&name, &version],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    if cap.get::<_, String>("schema_digest") != schema_digest {
        return Err(Error::conflict("Capability digest differs"));
    }
    let definition: Value = cap.get("spec");
    if !matches!(
        definition["side_effect"].as_str(),
        Some("none" | "idempotent")
    ) {
        return Err(Error::invalid(
            "This call profile requires supported idempotent semantics",
        ));
    }
    let bytes = serde_json::to_vec(&input["input"])?;
    if bytes.len() > unsigned(&definition["maximum_input_bytes"])? as usize {
        return Err(Error::invalid("Capability input exceeds its byte limit"));
    }
    validate_business(&definition["input_schema"], &input["input"])?;
    let fixed = input
        .get("provider_deployment_id")
        .map(|_| id(input, "provider_deployment_id"))
        .transpose()?;
    let installation = skill.as_ref().map(|s| s.installation_id);
    let providers=tx.query("SELECT i.id,i.generation,d.id AS deployment_id,d.owner_id,n.manifest FROM provider_records p JOIN instances i ON i.id=p.instance_id JOIN deployments d ON d.id=i.deployment_id JOIN installations n ON n.id=d.installation_id WHERE p.name=$1 AND p.version=$2 AND p.availability='ready' AND p.observed_at>now()-interval '1 millisecond'*((n.manifest->'health'->>'interval_ms')::bigint*(n.manifest->'health'->>'failure_threshold')::bigint+(n.manifest->'health'->>'timeout_ms')::bigint) AND i.phase='ready' AND d.spec->>'enabled'='true' AND d.archived_at IS NULL AND ($3::uuid IS NULL OR d.id=$3) AND ($4::uuid IS NULL OR n.id=$4) ORDER BY i.id FOR UPDATE OF i",&[&name,&version,&fixed,&installation]).await?;
    let row = providers
        .first()
        .ok_or_else(|| Error::unavailable("No healthy fixed provider is available"))?;
    let instance: Uuid = row.get("id");
    let generation: i64 = row.get("generation");
    let manifest: Value = row.get("manifest");
    if definition["side_effect"] == "idempotent" {
        crate::artifact::validate_runtime_profile(&manifest)?;
        if manifest["data_schema_version"] == "none"
            || !manifest["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|cap| {
                    cap["contract"] == *contract
                        && cap["side_effect"] == "idempotent"
                        && ["input_schema", "output_schema", "stream_item_schema"]
                            .iter()
                            .all(|field| cap[*field] == definition[*field])
                })
        {
            return Err(Error::invalid(
                "Idempotent data writes require the verified versioned plugin profile",
            ));
        }
    }
    if manifest["business_entry"] == "skill_required" && skill.is_none() {
        return Err(Error::denied());
    }
    let active:i64=tx.query_one("SELECT count(*) FROM calls c WHERE provider_instance_id=$1 AND (state IN ('accepted','running') OR EXISTS(SELECT 1 FROM reservations r WHERE r.id=c.id AND r.kind='call' AND r.status<>'released'))",&[&instance]).await?.get(0);
    let max = manifest["concurrency"]["max_active_requests"]
        .as_i64()
        .unwrap_or(1);
    if active >= max {
        return Err(Error::exhausted());
    }
    let accounts = account_chain(&tx, ctx.subject).await?;
    let today = chrono::Utc::now().date_naive();
    for account in &accounts {
        let quota = tx
            .query_one(
                "SELECT * FROM quotas WHERE account_id=$1 FOR UPDATE",
                &[account],
            )
            .await?;
        let q: Value = quota.get("spec");
        let daily = if quota.get::<_, chrono::NaiveDate>("daily_bucket") == today {
            quota.get::<_, i64>("daily_calls_used")
        } else {
            0
        };
        if quota.get::<_, i64>("concurrent_calls_used")
            >= q["maximum_concurrent_calls"].as_i64().unwrap()
            || daily >= q["daily_calls"].as_i64().unwrap()
        {
            return Err(Error::exhausted());
        }
        tx.execute("UPDATE quotas SET concurrent_calls_used=concurrent_calls_used+1,daily_calls_used=$2::bigint+1,daily_bucket=$3 WHERE account_id=$1", &[account,&daily,&today]).await?;
    }
    let mut charged = crate::identity::call_sources(
        &tx,
        &ctx,
        &Resource::new("capability", format!("{name}@{version}"), None),
    )
    .await?;
    if let Some(skill) = &skill {
        charged.extend(
            crate::identity::call_sources_for(
                &tx,
                &ctx,
                "skill.invoke",
                &Resource::new("skill", skill.id, Some(skill.owner)),
            )
            .await?,
        );
    }
    let call = Uuid::new_v4();
    let timeout = input
        .get("timeout_ms")
        .map(unsigned)
        .transpose()?
        .unwrap_or(30000);
    let deadline = started + chrono::Duration::milliseconds(timeout as i64);
    if deadline <= chrono::Utc::now() {
        return Err(Error::new(
            "DEADLINE_EXCEEDED",
            "Call admission exceeded its deadline",
            axum::http::StatusCode::GATEWAY_TIMEOUT,
        ));
    }
    tx.execute("INSERT INTO calls(id,owner_id,credential_id,provider_instance_id,contract,input_digest,idempotency_key,state,deadline_at,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,'running',$8,now())",&[&call,&ctx.subject,&ctx.credential,&instance,&contract,&digest(&bytes),&business_key,&deadline]).await?;
    if let Some(skill) = &skill {
        tx.execute(
            "UPDATE calls SET skill_id=$2,skill_definition_digest=$3 WHERE id=$1",
            &[&call, &skill.id, &skill.definition_digest],
        )
        .await?;
    }
    for credential in charged {
        tx.execute(
            "INSERT INTO call_credential_charges(call_id,credential_id) VALUES($1,$2)",
            &[&call, &credential],
        )
        .await?;
    }
    tx.execute("INSERT INTO reservations(id,account_id,kind,expires_at,status,created_at) VALUES($1,$2,'call',$3,'active',now())", &[&call,&ctx.subject,&deadline]).await?;
    let delegation = app
        .auth
        .issue_rpc(&tx, &ctx, call, instance, generation, deadline)
        .await?;
    crate::host_blobs::grant_skill_call(
        &tx,
        app,
        &ctx,
        call,
        row.get("deployment_id"),
        &manifest,
        &definition,
        &input["input"],
        skill
            .as_ref()
            .is_none_or(|s| s.definition["input_blobs"] == true),
    )
    .await?;
    if skill.is_some() {
        crate::host_blobs::check_call_sources(&tx, &ctx, call).await?;
    }
    tx.execute("DELETE FROM idempotency_records WHERE subject_id=$1 AND credential_id=$2 AND method='POST' AND path=$4 AND key=$3 AND expires_at<=now()",&[&ctx.subject,&ctx.credential,&key,&path]).await?;
    tx.execute("INSERT INTO idempotency_records(subject_id,credential_id,method,path,key,fingerprint,resource_id,expires_at,created_at) VALUES($1,$2,'POST',$6,$3,$4,$5,now()+interval '24 hours',now())",&[&ctx.subject,&ctx.credential,&key,&fingerprint,&call.to_string(),&path]).await?;
    tx.commit().await?;
    let remaining =
        || Duration::from_millis((deadline - chrono::Utc::now()).num_milliseconds().max(0) as u64);
    let result = async {
        if let Some(stream) = stream {
            stream.bind(skill.as_ref().ok_or_else(Error::denied)?, call, deadline)?;
            stream.emit("accepted", json!({})).await?;
        }
        let address = app.runtime.address(instance)?;
        let channel = tls::channel(&app.config, instance, generation, &address).await?;
        let mut client =
            CapabilityServiceClient::new(channel).max_decoding_message_size(if stream.is_some() {
                1024 * 1024 + 1024 // One bounded JSON block plus protobuf metadata.
            } else {
                16778240
            });
        let mut context = controller::context(Some(ctx.subject));
        context.request_id = ctx.request.to_string();
        context.delegation_token = delegation;
        context.deadline_unix_ms = deadline.timestamp_millis();
        context.idempotency_key = business_key.to_owned();
        let mut request = tonic::Request::new(rpc::InvokeRequest {
            context: Some(context),
            call_id: call.to_string(),
            contract: Some(rpc::ContractRef {
                name: name.to_owned(),
                version: version.to_owned(),
                schema_digest: schema_digest.to_owned(),
            }),
            input_digest: digest(&bytes),
            input_json: bytes,
            binding_id: String::new(),
        });
        if skill.is_some() {
            check_live_call(app, token, peer, request_id, call, instance, generation).await?;
        }
        request.set_timeout(remaining());
        let response = if let Some(output) = stream {
            let mut response_stream = client.invoke_stream(request).await?.into_inner();
            let mut sequence = 0_u64;
            let mut completed = None;
            while let Some(chunk) = response_stream.message().await? {
                if chunk.call_id != call.to_string()
                    || chunk.sequence != sequence + 1
                    || completed.is_some()
                {
                    return Err(Error::conflict(
                        "Plugin stream identity, sequence or terminal state differs",
                    ));
                }
                sequence = chunk.sequence;
                match chunk.body {
                    Some(rpc::invoke_chunk::Body::ItemJson(bytes)) => {
                        let item = strict_json(&bytes, 1024 * 1024)?;
                        validate_business(&definition["stream_item_schema"], &item)?;
                        let skill = skill.as_ref().ok_or_else(Error::denied)?;
                        validate_business(&skill.definition["stream_item_schema"], &item)?;
                        check_live_call(app, token, peer, request_id, call, instance, generation)
                            .await?;
                        output.emit("item", item).await?;
                    }
                    Some(rpc::invoke_chunk::Body::Completed(response)) => {
                        completed = Some(response)
                    }
                    Some(rpc::invoke_chunk::Body::Failed(_)) => {
                        return Err(Error::unavailable("Plugin stream failed; inspect the Call"))
                    }
                    Some(rpc::invoke_chunk::Body::Canceled(_)) => {
                        return Err(Error::new(
                            "CANCELED",
                            "Plugin stream was canceled",
                            axum::http::StatusCode::CONFLICT,
                        ))
                    }
                    None => return Err(Error::conflict("Plugin stream chunk has no body")),
                }
            }
            completed
                .ok_or_else(|| Error::conflict("Plugin stream ended without a terminal result"))?
        } else {
            client.invoke(request).await?.into_inner()
        };
        if response.call_id != call.to_string()
            || digest(&response.output_json) != response.output_digest
        {
            return Err(Error::conflict(
                "Plugin returned a different call or output digest",
            ));
        }
        let output = strict_json(
            &response.output_json,
            unsigned(&definition["maximum_output_bytes"])? as usize,
        )?;
        validate_business(&definition["output_schema"], &output)?;
        if let Some(skill) = &skill {
            validate_business(&skill.definition["output_schema"], &output)?;
        }
        let mut out = json!({"call_id":call,"contract":contract,"provider_instance_id":instance,"output":output,"completed_at":chrono::Utc::now()});
        if let Some(skill) = &skill {
            out["skill_id"] = json!(skill.id);
            out["skill_definition_digest"] = json!(skill.definition_digest);
        }
        validate("CallResult", &out)?;
        if let Some(stream) = stream {
            stream.validate_completed(&out)?;
        }
        Ok(out)
    };
    let monitored = async {
        tokio::pin!(result);
        loop {
            tokio::select! {
                result = &mut result => return result,
                _ = async { if let Some(stream) = stream { stream.closed().await } else { std::future::pending::<()>().await } } => {
                    return Err(Error::new("CANCELED", "Skill stream consumer disconnected", axum::http::StatusCode::CONFLICT));
                }
                _ = tokio::time::sleep(Duration::from_millis(100)), if skill.is_some() => {
                    // Revoke in-flight work promptly. Retain its physical slot until
                    // the controller confirms process fencing, as for RPC timeouts.
                    check_live_call(app, token, peer, request_id, call, instance, generation).await?;
                }
            }
        }
    };
    let mut result = tokio::time::timeout(remaining(), monitored)
        .await
        .unwrap_or_else(|_| {
            Err(Error::new(
                "DEADLINE_EXCEEDED",
                "Call deadline elapsed; inspect the call record",
                axum::http::StatusCode::GATEWAY_TIMEOUT,
            ))
        });
    if app.leader.is_closed() {
        return Err(Error::unavailable("Controller leadership is lost").with_resource(call));
    }
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::call_lock(&tx).await?;
    let check = async {
        let current = app.auth.authenticate(&tx, token, peer, request_id).await?;
        if let Some(skill) = &skill {
            crate::host_blobs::check_call_sources(&tx, &current, call).await?;
            let resolved = crate::skills::resolve_for_invoke(&tx, &current, skill.id).await?;
            if resolved.definition_digest != skill.definition_digest
                || resolved.installation_id != skill.installation_id
            {
                return Err(Error::conflict("Fixed Skill binding changed"));
            }
        }
        authorize(
            &tx,
            &current,
            "capability.invoke",
            &Resource::new("capability", format!("{name}@{version}"), None),
        )
        .await
    };
    if let Err(e) = check.await {
        result = Err(e);
    }
    let (state, value, error) = match &result {
        Ok(v) => ("succeeded", Some(v.clone()), None),
        Err(e) => ("unknown", None, Some(e.detail())),
    };
    let changed = tx.execute("UPDATE calls SET state=$2,result=$3,error=$4,completed_at=CASE WHEN $2='succeeded' THEN now() ELSE NULL END WHERE id=$1 AND state='running'",&[&call,&state,&value,&error]).await?;
    if changed != 1 {
        let saved = tx
            .query_one("SELECT error FROM calls WHERE id=$1", &[&call])
            .await?
            .get::<_, Option<Value>>(0);
        if saved
            .as_ref()
            .is_some_and(|error| error["code"] == "CANCELED")
        {
            return Err(Error::new(
                "CANCELED",
                "Call cancellation was requested; physical outcome remains unknown",
                axum::http::StatusCode::CONFLICT,
            )
            .with_resource(call));
        }
        return Err(Error::new(
            "OUTCOME_UNKNOWN",
            "Call was fenced by recovery",
            axum::http::StatusCode::CONFLICT,
        )
        .with_resource(call));
    }
    // An error/timeout does not prove that the provider stopped executing.
    // Recovery fences its process before returning the reserved call slot.
    if result.is_ok() {
        tx.execute(
            "UPDATE reservations SET status='released' WHERE id=$1 AND kind='call'",
            &[&call],
        )
        .await?;
        for account in &accounts {
            tx.execute("UPDATE quotas SET concurrent_calls_used=greatest(0,concurrent_calls_used-1) WHERE account_id=$1", &[account]).await?;
        }
    }
    tx.execute(
        "UPDATE rpc_delegations SET status='revoked' WHERE call_id=$1",
        &[&call],
    )
    .await?;
    tx.execute("UPDATE execution_delegations SET status='revoked' WHERE id IN(SELECT delegation_id FROM calls WHERE id=$1)", &[&call]).await?;
    db::audit(
        &tx,
        ctx.subject,
        Some(ctx.credential),
        request_id,
        if skill.is_some() {
            "skill.invoke"
        } else {
            "capability.invoke"
        },
        &Resource::new("call", call, Some(ctx.subject)).json(),
        if result.is_ok() {
            "succeeded"
        } else {
            "failed"
        },
        if result.is_ok() {
            "CALL_COMPLETED"
        } else {
            "OUTCOME_UNKNOWN"
        },
    )
    .await?;
    tx.commit().await?;
    skill_result(skill.as_ref(), result.map_err(|e| e.with_resource(call))?)
}

#[allow(clippy::too_many_arguments)]
async fn check_live_call(
    app: &App,
    token: &str,
    peer: IpAddr,
    request_id: Uuid,
    call: Uuid,
    instance: Uuid,
    generation: i64,
) -> Result<()> {
    let mut connection = app.db.connect().await?;
    let check = connection.transaction().await?;
    let current = app
        .auth
        .authenticate(&check, token, peer, request_id)
        .await?;
    check_skill_authority(&check, &current, call).await?;
    crate::host_blobs::check_call_sources(&check, &current, call).await?;
    let status = check
        .query_one("SELECT state,error FROM calls WHERE id=$1", &[&call])
        .await?;
    if status
        .get::<_, Option<Value>>("error")
        .as_ref()
        .is_some_and(|error| error["code"] == "CANCELED")
    {
        return Err(Error::new(
            "CANCELED",
            "Call cancellation was requested; physical outcome remains unknown",
            axum::http::StatusCode::CONFLICT,
        ));
    }
    if status.get::<_, String>("state") != "running" {
        return Err(Error::conflict("Admitted call is no longer running"));
    }
    let live: bool = check.query_one("SELECT EXISTS(SELECT 1 FROM instances i JOIN deployments d ON d.id=i.deployment_id WHERE i.id=$1 AND i.generation=$2 AND i.phase='ready' AND d.archived_at IS NULL AND d.spec->>'enabled'='true')", &[&instance,&generation]).await?.get(0);
    if !live {
        return Err(Error::unavailable("Fixed Skill deployment is unavailable"));
    }
    check.commit().await?;
    Ok(())
}

/// HostService reads authority from the admitted Call, not plugin JSON/headers.
pub(crate) async fn check_skill_authority(
    c: &(impl tokio_postgres::GenericClient + Sync),
    ctx: &crate::auth::Context,
    call: Uuid,
) -> Result<()> {
    let row = c.query_opt(
        "SELECT c.skill_id,c.skill_definition_digest,d.installation_id FROM calls c LEFT JOIN instances i ON i.id=c.provider_instance_id LEFT JOIN deployments d ON d.id=i.deployment_id WHERE c.id=$1",
        &[&call],
    ).await?.ok_or_else(Error::denied)?;
    if let Some(skill) = row.get::<_, Option<Uuid>>("skill_id") {
        let resolved = crate::skills::resolve_for_invoke(c, ctx, skill).await?;
        if Some(resolved.definition_digest)
            != row.get::<_, Option<String>>("skill_definition_digest")
            || Some(resolved.installation_id) != row.get::<_, Option<Uuid>>("installation_id")
        {
            return Err(Error::denied());
        }
    }
    Ok(())
}

fn skill_result(skill: Option<&crate::skills::ResolvedSkill>, call: Value) -> Result<Value> {
    if let Some(skill) = skill {
        let value = json!({"skill_id":skill.id,"skill_version":skill.definition["version"],"definition_digest":skill.definition_digest,"call":call});
        validate("SkillCallResult", &value)?;
        Ok(value)
    } else {
        Ok(call)
    }
}

#[cfg(test)]
#[path = "broker_schema_tests.rs"]
mod schema_tests;
