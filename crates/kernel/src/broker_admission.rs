//! Authorization and the active-slot transaction for Capability RPCs.
use super::{
    business_slot_available, reserve_business, reserve_call_quota, skill_result, unary_failure,
};
use crate::{
    api::App,
    auth::{authorize, Context, CurrentAuthority, Resource},
    broker_queue::{self, Instance, Ticket},
    contracts::{digest, id, string, unsigned, validate, validate_business},
    db,
    error::{Error, Result},
    host_blobs, host_calls,
    policy::Path,
    skills::ResolvedSkill,
    task_engine::{self, AttemptLease, BrokerLeaseUse},
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::{net::IpAddr, time::Duration};
use tokio::time::{timeout_at, Instant};
use tokio_postgres::{GenericClient, Row, Transaction};
use uuid::Uuid;

pub(super) struct Request<'a> {
    pub app: &'a App,
    pub token: &'a str,
    pub peer: IpAddr,
    pub request_id: Uuid,
    pub input: &'a Value,
    pub key: &'a str,
    pub skill_id: Option<Uuid>,
    pub stream: Option<&'a crate::skill_streams::Output>,
    pub started: DateTime<Utc>,
    pub deadline_bound: Instant,
    pub browser_origin: Option<&'a str>,
}

pub(super) enum Admission {
    Replay(Value),
    Dispatch(Box<Call>),
}

pub(super) enum TaskAdmission {
    Existing(Uuid),
    Dispatch(Box<Call>),
}

pub(super) struct Call {
    pub ctx: Context,
    pub skill: Option<ResolvedSkill>,
    pub instance: Instance,
    pub deployment: Uuid,
    pub contract: Value,
    pub definition: Value,
    pub bytes: Vec<u8>,
    pub deadline: DateTime<Utc>,
    pub protected: bool,
    pub terminal: Option<unary_failure::Binding>,
    pub id: Uuid,
    pub delegation: String,
    pub accounts: Vec<Uuid>,
    pub business_key: String,
}

struct WaitingCall {
    ticket: Ticket,
    subject: Uuid,
    credential: Uuid,
    skill_digest: String,
    installation: Uuid,
    deployment: Uuid,
    sources: Vec<Path>,
}

struct Candidate<'tx, 'db> {
    ctx: Context,
    skill: Option<ResolvedSkill>,
    instance: Instance,
    deployment: Uuid,
    manifest: Value,
    contract: Value,
    definition: Value,
    capability: Resource,
    deadline: DateTime<Utc>,
    protected: bool,
    terminal: Option<unary_failure::Binding>,
    authority: Option<CurrentAuthority<'tx, 'db>>,
}

enum Decision<'tx, 'db> {
    Replay(Value),
    Rejected(Error),
    Retry,
    Enqueue,
    Wait,
    Ready(Box<Candidate<'tx, 'db>>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Attempt {
    Initial,
    Preview,
    Admit,
}

impl Request<'_> {
    pub(super) fn path(&self) -> String {
        self.skill_id.map_or_else(
            || "/api/platform/v1/calls".to_owned(),
            |id| {
                format!(
                    "/api/platform/v1/skills/{id}/calls{}",
                    if self.stream.is_some() { ":stream" } else { "" }
                )
            },
        )
    }

    pub(super) async fn admit(&self, path: &str) -> Result<Admission> {
        validate(
            if self.skill_id.is_some() {
                "SkillInvokeRequest"
            } else {
                "InvokeRequest"
            },
            self.input,
        )?;
        let input_bytes = serde_json::to_vec(&self.input["input"])?.len();
        let fingerprint = digest(&serde_json::to_vec(self.input)?);
        let mut waiting: Option<WaitingCall> = None;
        let mut inspect_queue = self.can_enqueue_without_ledger();
        let mut enqueue = false;
        loop {
            let background = if let Some(waiter) = waiting.as_ref() {
                waiter.ticket.background_permit().await?
            } else {
                None
            };
            let until = waiting
                .as_ref()
                .map_or(self.deadline_bound, |waiter| waiter.ticket.deadline);
            let expired = if waiting.is_some() {
                broker_queue::expired
            } else {
                call_expired
            };
            let now = Instant::now();
            if now >= until {
                return Err(expired());
            }
            let mut c = timeout_at(until, self.app.db.connect_scoped(until - now))
                .await
                .map_err(|_| expired())??;
            let initial = std::mem::take(&mut inspect_queue);
            let attempt = if std::mem::take(&mut enqueue)
                || initial
                    && timeout_at(until, self.queue_busy(&*c))
                        .await
                        .map_err(|_| expired())?
                        .unwrap_or(false)
            {
                Attempt::Preview
            } else if initial {
                Attempt::Initial
            } else {
                Attempt::Admit
            };
            let tx = timeout_at(until, c.transaction())
                .await
                .map_err(|_| expired())??;
            let decision = timeout_at(
                until,
                self.prepare(&tx, &mut waiting, path, &fingerprint, input_bytes, attempt),
            )
            .await
            .map_err(|_| expired())??;
            let candidate = match decision {
                Decision::Replay(result) => return Ok(Admission::Replay(result)),
                Decision::Rejected(error) => {
                    tx.commit().await?;
                    return Err(error);
                }
                Decision::Retry | Decision::Enqueue => {
                    enqueue = matches!(decision, Decision::Enqueue);
                    tx.rollback().await?;
                    continue;
                }
                Decision::Wait => {
                    tx.rollback().await?;
                    drop(c);
                    drop(background);
                    waiting
                        .as_ref()
                        .expect("registered waiter")
                        .ticket
                        .wait()
                        .await;
                    continue;
                }
                Decision::Ready(candidate) => candidate,
            };

            // Keep the signed wait deadline through staging. A timed-out
            // transaction rolls back without a Call or any quota charge.
            let call = timeout_at(until, self.record(&tx, *candidate, path, &fingerprint))
                .await
                .map_err(|_| expired())??;
            if waiting.is_some() {
                crate::skill_streams::ensure_active(self.app)?;
            }
            if Instant::now() >= until {
                return Err(expired());
            }
            tx.commit().await?;
            return Ok(Admission::Dispatch(Box::new(call)));
        }
    }

    fn can_enqueue_without_ledger(&self) -> bool {
        self.skill_id.is_some() && self.stream.is_none() && !self.token.starts_with("host:")
    }

    /// This lookup only chooses the read-only waiting path. All selections and
    /// permissions are checked in the following authorization transaction.
    async fn queue_busy(&self, c: &(impl GenericClient + Sync)) -> Result<bool> {
        let row = c.query_opt(
            "SELECT s.definition,s.installation_id,n.manifest->'concurrency'->'max_queue_requests' AS queue_limit
               FROM skills s JOIN installations n ON n.id=s.installation_id WHERE s.id=$1",
            &[&self.skill_id],
        ).await?;
        let Some(row) = row else { return Ok(false) };
        if row
            .get::<_, Option<Value>>("queue_limit")
            .and_then(|limit| limit.as_u64())
            .unwrap_or(0)
            == 0
        {
            return Ok(false);
        }
        let definition: Value = row.get("definition");
        let contract = &definition["target"]["contract"];
        let fixed = self
            .input
            .get("provider_deployment_id")
            .map(|_| id(self.input, "provider_deployment_id"))
            .transpose()?;
        let row = provider(
            c,
            string(contract, "name")?,
            string(contract, "version")?,
            fixed,
            Some(row.get("installation_id")),
            None,
            ProviderUse::Ordinary { lock: false },
        )
        .await?;
        let instance = Instance {
            id: row.get("id"),
            generation: row.get("generation"),
        };
        Ok(self.app.runtime.business_queue.has_waiters(instance)
            || !business_slot_available(c, instance.id, &row.get("manifest")).await?)
    }

    async fn record(
        &self,
        tx: &Transaction<'_>,
        candidate: Candidate<'_, '_>,
        path: &str,
        fingerprint: &str,
    ) -> Result<Call> {
        let business_key = self.input["idempotency_key"].as_str().unwrap_or(self.key);
        let staged = record_call(
            tx,
            self.app,
            candidate,
            &self.input["input"],
            business_key,
            Some(self.token),
        )
        .await?;
        let ctx = &staged.ctx;
        let call = staged.id;
        tx.execute("DELETE FROM idempotency_records WHERE subject_id=$1 AND credential_id=$2 AND method='POST' AND path=$4 AND key=$3 AND expires_at<=now()",&[&ctx.subject,&ctx.credential,&self.key,&path]).await?;
        tx.execute("INSERT INTO idempotency_records(subject_id,credential_id,method,path,key,fingerprint,resource_id,expires_at,created_at) VALUES($1,$2,'POST',$6,$3,$4,$5,now()+interval '24 hours',now())",&[&ctx.subject,&ctx.credential,&self.key,&fingerprint,&call.to_string(),&path]).await?;
        if let Some(stream) = self.stream {
            stream.ensure_open()?;
        }
        Ok(staged)
    }

    async fn prepare<'tx, 'db>(
        &self,
        tx: &'tx Transaction<'db>,
        waiting: &mut Option<WaitingCall>,
        path: &str,
        fingerprint: &str,
        input_bytes: usize,
        attempt: Attempt,
    ) -> Result<Decision<'tx, 'db>> {
        let app = self.app;
        if waiting.is_some() {
            crate::skill_streams::ensure_active(app)?;
        }
        let front = waiting
            .as_ref()
            .is_none_or(|waiter| waiter.ticket.is_head());
        let preview = attempt == Attempt::Preview;
        let can_admit = front && !preview;
        if can_admit {
            db::call_lock(tx).await?;
            if attempt == Attempt::Initial && self.queue_busy(tx).await.unwrap_or(false) {
                // Another admission may have filled the last slot while this
                // request waited for the ledger. Authorize its wait in a new
                // transaction so active Calls can settle in the meantime.
                return Ok(Decision::Enqueue);
            }
        } else {
            // Waiting-only work checks live binding and authority without
            // serializing behind the durable Call ledger.
            db::auth_lock(tx, false).await?;
        }
        let mut ctx =
            host_calls::authenticate(tx, app, self.token, self.peer, self.request_id).await?;
        ctx.browser_origin = self.browser_origin.map(str::to_owned);
        let skill = if let Some(id) = self.skill_id {
            match crate::skills::resolve_for_invoke(tx, &ctx, id).await {
                Ok(skill) => Some(skill),
                Err(error) => {
                    db::audit(
                        tx,
                        ctx.subject,
                        Some(ctx.credential),
                        self.request_id,
                        "skill.invoke",
                        &Resource::new("skill", id, None).json(),
                        "denied",
                        error.code,
                    )
                    .await?;
                    return Ok(Decision::Rejected(error));
                }
            }
        } else {
            None
        };
        let input = &self.input["input"];
        let timeout = self.input.get("timeout_ms").map(unsigned).transpose()?;
        let (contract, timeout) = if let Some(skill) = &skill {
            let maximum = unsigned(&skill.definition["maximum_timeout_ms"])?;
            let timeout = timeout.unwrap_or(120000.min(maximum));
            if timeout > maximum {
                return Err(Error::invalid(
                    "Requested deadline exceeds the fixed Skill maximum",
                ));
            }
            if front {
                validate_business(&skill.definition["input_schema"], input)?;
            }
            (&skill.definition["target"]["contract"], timeout)
        } else {
            (&self.input["contract"], timeout.unwrap_or(30000))
        };
        let name = string(contract, "name")?;
        let version = string(contract, "version")?;
        let schema_digest = string(contract, "schema_digest")?;
        let capability = Resource::new("capability", format!("{name}@{version}"), None);
        if let Err(error) = authorize(tx, &ctx, "capability.invoke", &capability).await {
            db::audit(
                tx,
                ctx.subject,
                Some(ctx.credential),
                self.request_id,
                "capability.invoke",
                &capability.json(),
                "denied",
                error.code,
            )
            .await?;
            return Ok(Decision::Rejected(error));
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
            Some("none" | "idempotent" | "queryable" | "non_repeatable")
        ) {
            return Err(Error::invalid(
                "Unsupported capability side-effect semantics",
            ));
        }
        if self.stream.is_some() && definition["stream_item_schema"].is_null() {
            return Err(Error::invalid(
                "Capability does not declare a streaming item schema",
            ));
        }
        let mut protected = skill.is_some()
            || definition["side_effect"] != "none"
            || self.token.starts_with("host:");
        let fixed = self
            .input
            .get("provider_deployment_id")
            .map(|_| id(self.input, "provider_deployment_id"))
            .transpose()?;
        let installation = skill.as_ref().map(|s| s.installation_id);

        let mut authority = None;
        let mut pinned = None;
        if let Some(waiter) = waiting.as_ref() {
            let skill = skill.as_ref().ok_or_else(Error::denied)?;
            if ctx.subject != waiter.subject
                || ctx.credential != waiter.credential
                || skill.definition_digest != waiter.skill_digest
                || skill.installation_id != waiter.installation
            {
                return Err(Error::denied());
            }
            let proof = CurrentAuthority::new(tx, &ctx)
                .await?
                .restrict_to(tx, &ctx, &waiter.sources)
                .await?;
            proof.require(tx, &ctx, "skill.invoke", &skill_resource(skill))?;
            proof.require(tx, &ctx, "capability.invoke", &capability)?;
            if front {
                // Disable/upgrade locks the deployment before its instances.
                tx.query_opt(
                    "SELECT id FROM deployments WHERE id=$1 AND spec->>'enabled'='true' AND archived_at IS NULL FOR SHARE",
                    &[&waiter.deployment],
                ).await?.ok_or_else(|| Error::unavailable("Queued deployment is no longer enabled"))?;
            }
            let row = provider(
                tx,
                name,
                version,
                Some(waiter.deployment),
                installation,
                Some(waiter.ticket.instance),
                ProviderUse::Ordinary { lock: front },
            )
            .await?;
            crate::dependencies::check_ready(tx, app, row.get("deployment_id")).await?;
            pinned = Some(row);
            authority = Some(proof);
        }

        let prior = if can_admit {
            let lock = format!(
                "call:{}:{}:{path}:{}",
                ctx.subject, ctx.credential, self.key
            );
            tx.query_one(
                "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
                &[&lock],
            )
            .await?;
            tx.query_opt("SELECT * FROM idempotency_records WHERE subject_id=$1 AND credential_id=$2 AND method='POST' AND path=$4 AND key=$3 AND expires_at>now() FOR UPDATE", &[&ctx.subject,&ctx.credential,&self.key,&path]).await?
        } else if preview && tx.query_one(
            "SELECT EXISTS(SELECT 1 FROM idempotency_records WHERE subject_id=$1 AND credential_id=$2 AND method='POST' AND path=$4 AND key=$3 AND expires_at>now())",
            &[&ctx.subject,&ctx.credential,&self.key,&path],
        ).await?.get(0) {
            return Ok(Decision::Retry);
        } else {
            None
        };
        if let Some(r) = prior {
            if r.get::<_, String>("fingerprint") != fingerprint {
                return Err(Error::new(
                    "IDEMPOTENCY_CONFLICT",
                    "The key was already used for different input",
                    axum::http::StatusCode::CONFLICT,
                ));
            }
            let call = r
                .get::<_, Option<String>>("resource_id")
                .and_then(|s| s.parse::<Uuid>().ok())
                .ok_or_else(|| Error::conflict("Missing prior call"))?;
            if self.stream.is_some() {
                return Err(Error::conflict(
                    "Generation streams cannot be replayed; inspect the admitted Call",
                )
                .with_resource(call));
            }
            let row = tx
                .query_one("SELECT state,result,error FROM calls WHERE id=$1", &[&call])
                .await?;
            let state: String = row.get("state");
            let failure = r
                .get::<_, Option<i32>>("response_status")
                .filter(|_| state == "failed");
            if state == "succeeded" || failure.is_some() {
                let deployment: Uuid = tx.query_one(
                    "SELECT i.deployment_id FROM calls c JOIN instances i ON i.id=c.provider_instance_id WHERE c.id=$1",
                    &[&call],
                ).await?.get(0);
                crate::dependencies::check_ready(tx, app, deployment).await?;
                if protected
                    || tx
                        .query_one(
                            "SELECT EXISTS(SELECT 1 FROM host_rpc_contexts WHERE call_id=$1)",
                            &[&call],
                        )
                        .await?
                        .get::<_, bool>(0)
                {
                    super::check_skill_authority(tx, &ctx, call).await?;
                }
                if let Some(status) = failure {
                    let error = unary_failure::replay(call, row.get("error"), status)?;
                    return Ok(Decision::Rejected(error));
                }
                let value = row
                    .get::<_, Option<Value>>("result")
                    .ok_or_else(|| Error::conflict("Call result is unavailable"))?;
                return skill_result(skill.as_ref(), value).map(Decision::Replay);
            }
            return Err(Error::new(
                "OUTCOME_UNKNOWN",
                "Prior call is not a confirmed success; query its call ID",
                axum::http::StatusCode::CONFLICT,
            )
            .with_resource(call));
        }
        if input_bytes
            > app
                .config
                .json_limit(unsigned(&definition["maximum_input_bytes"])? as usize)
        {
            return Err(Error::invalid("Capability input exceeds its byte limit"));
        }
        if front {
            validate_business(&definition["input_schema"], input)?;
        }
        let row = if let Some(row) = pinned {
            row
        } else {
            let row = provider(
                tx,
                name,
                version,
                fixed,
                installation,
                None,
                ProviderUse::Ordinary { lock: can_admit },
            )
            .await?;
            crate::dependencies::check_ready(tx, app, row.get("deployment_id")).await?;
            row
        };
        let instance = Instance {
            id: row.get("id"),
            generation: row.get("generation"),
        };
        let deployment = row.get("deployment_id");
        let manifest: Value = row.get("manifest");
        protected |= manifest["dependencies"]
            .as_array()
            .is_some_and(|d| !d.is_empty());
        let signed = front && unary_failure::signed_capability(&manifest, contract, &definition);
        if front && definition["side_effect"] != "none" {
            crate::artifact::validate_runtime_profile(&manifest)?;
            if !signed {
                return Err(Error::invalid(
                    "Side-effecting execution requires its exact signed capability",
                ));
            }
        }
        if manifest["business_entry"] == "skill_required" && skill.is_none() {
            return Err(Error::denied());
        }
        let has_proxy = manifest["requested_rules"].as_array().is_some_and(|rules| {
            rules.iter().any(|request| {
                host_blobs::proxy_action(
                    request["rule"]["action"].as_str().unwrap_or(""),
                    request["rule"]["resource"]["kind"].as_str().unwrap_or(""),
                )
            })
        });
        protected |= has_proxy;
        let deadline = host_calls::bound_deadline(
            tx,
            app,
            self.token,
            self.started + chrono::Duration::milliseconds(timeout as i64),
        )
        .await?;
        if deadline <= Utc::now() {
            return Err(call_expired());
        }
        let queue = &app.runtime.business_queue;
        let turn = waiting
            .as_ref()
            .map_or_else(|| !queue.has_waiters(instance), |_| front);
        if !turn || !business_slot_available(tx, instance.id, &manifest).await? {
            let limit = manifest["concurrency"]["max_queue_requests"]
                .as_u64()
                .unwrap_or(0);
            let skill = skill
                .as_ref()
                .filter(|_| {
                    limit > 0 && self.stream.is_none() && broker_queue::supports_manifest(&manifest)
                })
                .ok_or_else(Error::exhausted)?;
            if waiting.is_none() && !preview && self.can_enqueue_without_ledger() {
                // The provider became busy after the optimistic lookup. Let
                // Call finalizers through before expanding the queued proof.
                return Ok(Decision::Enqueue);
            }
            if has_proxy {
                let inputs = host_blobs::authorize_skill_blob_inputs(
                    tx,
                    app,
                    &ctx,
                    deployment,
                    &manifest,
                    &definition,
                    input,
                    skill.definition["input_blobs"] == true,
                )
                .await?;
                // Each authorized input already checked the current signed
                // approval. Without an input, check approval independently.
                if inputs.is_empty() {
                    host_blobs::check_approval(tx, app, deployment, &manifest).await?;
                }
            }
            if waiting.is_none() {
                let maximum =
                    Duration::from_millis(unsigned(&manifest["concurrency"]["queue_timeout_ms"])?);
                host_calls::check_queued_child(tx, app, self.token, instance.id).await?;
                let proof = CurrentAuthority::new(tx, &ctx).await?;
                let skill_resource = skill_resource(skill);
                let sources = proof
                    .paths(tx, &ctx, &capability)?
                    .iter()
                    .filter_map(|path| {
                        let rules = json!([&path.rule]);
                        let resource = if crate::auth::rules_allow(
                            &rules,
                            "capability.invoke",
                            &capability,
                            ctx.peer,
                        ) {
                            &capability
                        } else if crate::auth::rules_allow(
                            &rules,
                            "skill.invoke",
                            &skill_resource,
                            ctx.peer,
                        ) {
                            &skill_resource
                        } else {
                            return None;
                        };
                        let mut path = path.clone();
                        path.rule["resource"] = resource.json();
                        Some(path)
                    })
                    .collect();
                let remaining = (deadline - Utc::now())
                    .to_std()
                    .map_err(|_| call_expired())?;
                let deadline = self
                    .deadline_bound
                    .min(Instant::now() + maximum.min(remaining));
                let ticket = queue.enqueue(instance, limit as usize, deadline)?;
                *waiting = Some(WaitingCall {
                    ticket,
                    subject: ctx.subject,
                    credential: ctx.credential,
                    skill_digest: skill.definition_digest.clone(),
                    installation: skill.installation_id,
                    deployment,
                    sources,
                });
            }
            return Ok(Decision::Wait);
        }
        if preview {
            return Ok(Decision::Retry);
        }
        let terminal = (self.stream.is_none() && signed && definition["contract"] == *contract)
            .then(|| unary_failure::Binding {
                installation: row.get("installation_id"),
                manifest_digest: row.get("manifest_digest"),
                has_proxy,
            });
        Ok(Decision::Ready(Box::new(Candidate {
            contract: contract.clone(),
            ctx,
            skill,
            instance,
            deployment,
            manifest,
            definition,
            capability,
            deadline,
            protected,
            terminal,
            authority,
        })))
    }
}

const TASK_ADMISSION_TIMEOUT: Duration = Duration::from_secs(10);
const AUTOMATIC_CANDIDATES_PER_POLL: usize = 16;

pub(super) async fn admit_task(app: &App, lease: &AttemptLease) -> Result<TaskAdmission> {
    let limit = Instant::now() + TASK_ADMISSION_TIMEOUT;
    timeout_at(limit, admit_task_bounded(app, lease, limit))
        .await
        .map_err(|_| call_expired())?
}

async fn admit_task_bounded(
    app: &App,
    lease: &AttemptLease,
    limit: Instant,
) -> Result<TaskAdmission> {
    crate::skill_streams::ensure_active(app)?;
    let mut connection = app
        .db
        .connect_scoped(limit.saturating_duration_since(Instant::now()))
        .await?;
    let tx = connection.transaction().await?;
    db::call_lock(&tx).await?;
    let proof =
        task_engine::broker_dispatch_proof(&tx, &app.auth, lease, BrokerLeaseUse::Admission)
            .await?;
    if let Some(row) = task_call(&tx, lease, &proof, None).await? {
        let call = row.try_get("id")?;
        let cutoff = charged_task_cutoff(&tx, &proof.ctx, call).await?;
        task_engine::broker_remaining(&tx, proof.deadline.min(cutoff)).await?;
        crate::skill_streams::ensure_active(app)?;
        tx.commit().await?;
        return Ok(TaskAdmission::Existing(call));
    }
    if !proof.active {
        return Err(Error::conflict(
            "Task is no longer accepting new Call admission",
        ));
    }
    let staged = stage_task_call(&tx, app, lease, proof, limit).await?;
    tx.commit().await?;
    if !staged.may_send(app) {
        // COMMIT may have completed after the dispatch budget. The ledger is
        // still authoritative but this invocation must never send an RPC.
        return Ok(TaskAdmission::Existing(staged.call.id));
    }
    Ok(TaskAdmission::Dispatch(staged.call))
}

/// Select and admit one short fixed Capability step without publishing a
/// provisional Attempt separately from the Broker Call it will execute.
pub(super) async fn admit_next_task(
    app: &App,
    worker_id: Uuid,
    after: &mut Option<(DateTime<Utc>, Uuid)>,
) -> Result<Option<(AttemptLease, Box<Call>)>> {
    let limit = Instant::now() + TASK_ADMISSION_TIMEOUT;
    timeout_at(limit, admit_next_task_bounded(app, worker_id, after, limit))
        .await
        .map_err(|_| call_expired())?
}

async fn admit_next_task_bounded(
    app: &App,
    worker_id: Uuid,
    after: &mut Option<(DateTime<Utc>, Uuid)>,
    limit: Instant,
) -> Result<Option<(AttemptLease, Box<Call>)>> {
    crate::skill_streams::ensure_active(app)?;
    let mut connection = app
        .db
        .connect_scoped(limit.saturating_duration_since(Instant::now()))
        .await?;
    for _ in 0..AUTOMATIC_CANDIDATES_PER_POLL {
        crate::skill_streams::ensure_active(app)?;
        let tx = connection.transaction().await?;
        db::call_lock(&tx).await?;
        let Some(candidate) = task_engine::next_automatic_candidate(&tx, *after).await? else {
            tx.rollback().await?;
            *after = None;
            return Ok(None);
        };
        let step_id: Uuid = candidate.try_get("step_id")?;
        let created_at: DateTime<Utc> = candidate.try_get("step_created_at")?;
        let staging = async {
            let Some(lease) =
                task_engine::lease_automatic_candidate(&tx, &app.auth, &candidate, worker_id)
                    .await?
            else {
                return Ok(None);
            };
            let proof =
                task_engine::broker_dispatch_proof(&tx, &app.auth, &lease, BrokerLeaseUse::Send)
                    .await?;
            let staged = stage_task_call(&tx, app, &lease, proof, limit).await?;
            Ok::<_, Error>(Some((lease, staged)))
        }
        .await;
        match staging {
            Ok(Some((lease, staged))) => {
                tx.commit().await?;
                *after = None;
                if !staged.may_send(app) {
                    return Err(Error::new(
                        "OUTCOME_UNKNOWN",
                        "Task Call committed without a live transport budget",
                        axum::http::StatusCode::CONFLICT,
                    )
                    .with_resource(staged.call.id));
                }
                return Ok(Some((lease, staged.call)));
            }
            Ok(None) => {}
            Err(error) if skip_automatic_candidate(&error) => {
                tracing::debug!(%step_id, reason_code=error.code, "Task Call admission deferred");
            }
            Err(error) => return Err(error),
        }
        // All writes to the step, lease, quota and Call disappear together.
        tx.rollback().await?;
        *after = Some((created_at, step_id));
    }
    Ok(None)
}

fn skip_automatic_candidate(error: &Error) -> bool {
    matches!(
        error.code,
        "PERMISSION_DENIED" | "INVALID_CREDENTIAL" | "TASK_PROVIDER_UNAVAILABLE"
            | "RESOURCE_EXHAUSTED" | "QUOTA_EXCEEDED" | "DEADLINE_EXCEEDED"
            | "INVALID_ARGUMENT" | "STATE_CONFLICT" | "RESOURCE_NOT_FOUND"
    ) || matches!(error.message,
        "No healthy fixed provider is available"
            | "Admitted task provider or RPC binding is unavailable"
            | "Task dispatch requires an exact signed direct Capability without proxy or dependencies"
    )
}

struct StagedTaskCall {
    call: Box<Call>,
    sampled: Instant,
    remaining: Duration,
}

impl StagedTaskCall {
    fn may_send(&self, app: &App) -> bool {
        self.sampled.elapsed() < self.remaining && crate::skill_streams::ensure_active(app).is_ok()
    }
}

async fn stage_task_call(
    tx: &Transaction<'_>,
    app: &App,
    lease: &AttemptLease,
    proof: task_engine::BrokerDispatchProof,
    limit: Instant,
) -> Result<StagedTaskCall> {
    let prepared = task_engine::broker_input(tx, &proof).await?;
    let contract = &proof.contract;
    let name = string(contract, "name")?;
    let version = string(contract, "version")?;
    let digest = string(contract, "schema_digest")?;
    let capability = Resource::new("capability", format!("{name}@{version}"), None);
    let definition: Value = tx.query_opt(
        "SELECT spec FROM capability_contracts WHERE name=$1 AND version=$2 AND schema_digest=$3 FOR SHARE",
        &[&name,&version,&digest],
    ).await?.ok_or_else(Error::not_found)?.try_get(0)?;
    let row = provider(
        tx,
        name,
        version,
        Some(proof.deployment),
        Some(proof.installation),
        None,
        ProviderUse::FixedTask,
    )
    .await?;
    let instance = Instance {
        id: row.try_get("id")?,
        generation: row.try_get("generation")?,
    };
    let manifest: Value = row.try_get("manifest")?;
    crate::artifact::validate_runtime_profile(&manifest)?;
    let proxy = manifest["requested_rules"].as_array().is_some_and(|rules| {
        rules.iter().any(|request| {
            host_blobs::proxy_action(
                request["rule"]["action"].as_str().unwrap_or(""),
                request["rule"]["resource"]["kind"].as_str().unwrap_or(""),
            )
        })
    });
    if proxy
        || manifest["dependencies"]
            .as_array()
            .is_some_and(|deps| !deps.is_empty())
        || manifest
            .get("business_entry")
            .is_some_and(|entry| entry != "direct")
        || manifest["data_schema_version"] != "none"
        || definition["contract"] != *contract
        || !unary_failure::signed_capability(&manifest, contract, &definition)
    {
        return Err(Error::unavailable("Task dispatch requires an exact signed direct Capability without proxy or dependencies"));
    }
    if app.runtime.business_queue.has_waiters(instance) {
        return Err(Error::exhausted());
    }
    let input = &prepared.request["input"];
    if serde_json::to_vec(input)?.len()
        > app
            .config
            .json_limit(unsigned(&definition["maximum_input_bytes"])? as usize)
    {
        return Err(Error::invalid("Capability input exceeds its byte limit"));
    }
    validate_business(&definition["input_schema"], input)?;
    let authority = CurrentAuthority::new(tx, &proof.ctx)
        .await?
        .restrict_to_original_sources(tx, &proof.ctx, &proof.business_paths)
        .await?;
    authority.require(tx, &proof.ctx, "capability.invoke", &capability)?;
    let timeout = unsigned(&prepared.request["timeout_ms"])? as i64;
    let now: DateTime<Utc> = tx
        .query_one("SELECT clock_timestamp()", &[])
        .await?
        .try_get(0)?;
    let deadline = proof
        .deadline
        .min(now + chrono::Duration::milliseconds(timeout));
    let candidate = Candidate {
        ctx: proof.ctx,
        skill: None,
        instance,
        deployment: proof.deployment,
        manifest,
        contract: contract.clone(),
        definition,
        capability,
        deadline,
        protected: true,
        terminal: Some(unary_failure::Binding {
            installation: proof.installation,
            manifest_digest: row.try_get("manifest_digest")?,
            has_proxy: false,
        }),
        authority: Some(authority),
    };
    let call = record_call(tx, app, candidate, input, &proof.business_key, None).await?;
    tx.execute(
        "INSERT INTO task_attempt_calls(attempt_id,call_id,installation_id) VALUES($1,$2,$3)",
        &[&lease.attempt_id, &call.id, &proof.installation],
    )
    .await?;
    tx.execute(
        "UPDATE attempts SET state='running',revision=revision+1,updated_at=clock_timestamp() WHERE id=$1",
        &[&lease.attempt_id],
    ).await?;
    // Include waiting on the retained link, triggers and every ledger write.
    tx.batch_execute("SET CONSTRAINTS ALL IMMEDIATE").await?;
    let cutoff = validate_task_call(tx, app, lease, call.id, false).await?;
    let sampled = Instant::now();
    let remaining = task_engine::broker_remaining(tx, cutoff).await?;
    crate::skill_streams::ensure_active(app)?;
    if sampled.elapsed() >= remaining || Instant::now() >= limit {
        return Err(call_expired());
    }
    Ok(StagedTaskCall {
        call: Box::new(call),
        sampled,
        remaining,
    })
}

/// A monitor holds the shared authorization fence. Settlement also owns the
/// Call ledger. Validation never commits, rolls back, or changes any state.
pub(super) async fn validate_task_call(
    tx: &Transaction<'_>,
    app: &App,
    lease: &AttemptLease,
    call: Uuid,
    allow_cancel_requested: bool,
) -> Result<DateTime<Utc>> {
    crate::skill_streams::ensure_active(app)?;
    let purpose = if allow_cancel_requested {
        BrokerLeaseUse::Observe
    } else {
        BrokerLeaseUse::Send
    };
    let proof = task_engine::broker_dispatch_proof(tx, &app.auth, lease, purpose).await?;
    let call_row = task_call(tx, lease, &proof, Some(call))
        .await?
        .ok_or_else(Error::denied)?;
    if call_row.try_get::<_, &str>("state")? != "running" {
        return Err(Error::conflict("Admitted Call is no longer running"));
    }
    let name = string(&proof.contract, "name")?;
    let version = string(&proof.contract, "version")?;
    let rpc_until: DateTime<Utc> = tx.query_opt(
        "SELECT r.expires_at FROM calls c
           JOIN instances i ON i.id=c.provider_instance_id
           JOIN deployments d ON d.id=i.deployment_id
           JOIN installations n ON n.id=d.installation_id
           JOIN provider_records p ON p.instance_id=i.id AND p.name=$2 AND p.version=$3
           JOIN execution_delegations e ON e.id=c.delegation_id
           JOIN rpc_delegations r ON r.call_id=c.id AND r.execution_delegation_id=e.id
              AND r.instance_id=i.id AND r.generation=i.generation
          WHERE c.id=$1 AND d.id=$4 AND n.id=$5 AND i.phase='ready' AND i.health='alive'
            AND p.availability='ready' AND p.observed_generation=i.generation
            AND p.observed_at>clock_timestamp()-interval '1 millisecond'*
                ((n.manifest->'health'->>'interval_ms')::bigint*(n.manifest->'health'->>'failure_threshold')::bigint
                +(n.manifest->'health'->>'timeout_ms')::bigint)
            AND e.status='active' AND e.subject_id=c.owner_id AND e.issuer_credential_id=c.credential_id
            AND e.expires_at>=r.expires_at AND r.status='active'
            AND r.subject_id=c.owner_id AND r.issuer_credential_id=c.credential_id
          FOR SHARE OF i,p,e,r",
        &[&call, &name, &version, &proof.deployment, &proof.installation],
    ).await?.ok_or_else(|| Error::unavailable("Admitted task provider or RPC binding is unavailable"))?.try_get(0)?;
    let cutoff = charged_task_cutoff(tx, &proof.ctx, call).await?;
    let deadline = proof
        .deadline
        .min(call_row.try_get("deadline_at")?)
        .min(rpc_until)
        .min(cutoff);
    task_engine::broker_remaining(tx, deadline).await?;
    crate::skill_streams::ensure_active(app)?;
    Ok(deadline)
}

async fn charged_task_cutoff(
    tx: &Transaction<'_>,
    ctx: &Context,
    call: Uuid,
) -> Result<DateTime<Utc>> {
    let paths = host_blobs::checked_call_sources(tx, ctx, call).await?;
    task_engine::broker_paths_until(tx, ctx, &paths).await
}

async fn task_call(
    tx: &Transaction<'_>,
    lease: &AttemptLease,
    proof: &task_engine::BrokerDispatchProof,
    call: Option<Uuid>,
) -> Result<Option<Row>> {
    let row = tx
        .query_opt(
            "SELECT c.id,c.state,c.deadline_at,
            b.installation_id=$2 AND c.owner_id=$3 AND c.credential_id=$4 AND c.contract=$5
              AND c.idempotency_key=$6 AND i.deployment_id=$7
              AND c.skill_id IS NULL AND c.http_binding IS NULL AS matches
           FROM task_attempt_calls b JOIN calls c ON c.id=b.call_id
           LEFT JOIN instances i ON i.id=c.provider_instance_id
          WHERE b.attempt_id=$1 FOR SHARE OF c",
            &[
                &lease.attempt_id,
                &proof.installation,
                &proof.ctx.subject,
                &proof.ctx.credential,
                &proof.contract,
                &proof.business_key,
                &proof.deployment,
            ],
        )
        .await?;
    if let Some(row) = &row {
        if row.try_get::<_, Option<bool>>("matches")? != Some(true)
            || call.is_some_and(|id| row.get::<_, Uuid>("id") != id)
        {
            return Err(Error::denied());
        }
    }
    Ok(row)
}

/// Both callers reach this point with the live provider and the Call ledger
/// locked. Worker dispatch uses an independent per-Attempt association and an
/// exact task proof. Ordinary requests retain their existing HTTP replay gate.
async fn record_call(
    tx: &Transaction<'_>,
    app: &App,
    candidate: Candidate<'_, '_>,
    input: &Value,
    business_key: &str,
    token: Option<&str>,
) -> Result<Call> {
    let Candidate {
        ctx,
        skill,
        instance,
        deployment,
        manifest,
        contract,
        definition,
        capability,
        deadline,
        protected,
        terminal,
        authority,
    } = candidate;
    let (accounts, charged) = if token.is_some() {
        reserve_call_quota(
            tx,
            &ctx,
            "capability.invoke",
            &capability,
            skill.as_ref(),
            authority.as_ref(),
        )
        .await?
    } else {
        reserve_business(
            tx,
            &ctx,
            instance.id,
            &manifest,
            "capability.invoke",
            &capability,
            None,
            authority.as_ref(),
        )
        .await?
    };
    let deadline = if token.is_some() {
        deadline
    } else {
        let authority = authority.as_ref().ok_or_else(Error::denied)?;
        let paths = host_blobs::retain_charged_paths(
            tx,
            &ctx,
            authority.paths(tx, &ctx, &capability)?.to_vec(),
            &charged,
            "capability.invoke",
        )
        .await?;
        deadline.min(task_engine::broker_paths_until(tx, &ctx, &paths).await?)
    };
    let call = if let Some(token) = token {
        host_calls::new_call(tx, app, token).await?
    } else {
        Uuid::new_v4()
    };
    if deadline <= Utc::now() {
        return Err(call_expired());
    }
    let bytes = serde_json::to_vec(input)?;
    tx.execute("INSERT INTO calls(id,owner_id,credential_id,provider_instance_id,contract,input_digest,idempotency_key,state,deadline_at,created_at)
        VALUES($1,$2,$3,$4,$5,$6,$7,'running',$8,now())",
        &[&call,&ctx.subject,&ctx.credential,&instance.id,&contract,&digest(&bytes),&business_key,&deadline]).await?;
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
    tx.execute(
        "INSERT INTO reservations(id,account_id,kind,expires_at,status,created_at)
        VALUES($1,$2,'call',$3,'active',now())",
        &[&call, &ctx.subject, &deadline],
    )
    .await?;
    if let Some(token) = token {
        host_calls::bind_child(tx, app, token, call, instance.id).await?;
    }
    let delegation = app
        .auth
        .issue_rpc(tx, &ctx, call, instance.id, instance.generation, deadline)
        .await?;
    if token.is_some() {
        host_blobs::grant_skill_call(
            tx,
            app,
            &ctx,
            call,
            deployment,
            &manifest,
            &definition,
            input,
            skill
                .as_ref()
                .is_none_or(|s| s.definition["input_blobs"] == true),
        )
        .await?;
        if protected {
            host_blobs::check_call_sources(tx, &ctx, call).await?;
        }
    } else {
        host_blobs::grant_task_call(
            tx,
            &ctx,
            call,
            authority.as_ref().ok_or_else(Error::denied)?,
        )
        .await?;
    }
    Ok(Call {
        ctx,
        skill,
        instance,
        deployment,
        contract,
        definition,
        bytes,
        deadline,
        protected,
        terminal,
        id: call,
        delegation,
        accounts,
        business_key: business_key.to_owned(),
    })
}

fn skill_resource(skill: &ResolvedSkill) -> Resource {
    Resource::new("skill", skill.id, Some(skill.owner))
}

enum ProviderUse {
    Ordinary { lock: bool },
    FixedTask,
}

async fn provider(
    c: &(impl GenericClient + Sync),
    name: &str,
    version: &str,
    fixed: Option<Uuid>,
    installation: Option<Uuid>,
    pinned: Option<Instance>,
    selection: ProviderUse,
) -> Result<Row> {
    let instance = pinned.map(|instance| instance.id);
    let generation = pinned.map(|instance| instance.generation);
    let fixed_task = matches!(selection, ProviderUse::FixedTask);
    let lock = match selection {
        ProviderUse::Ordinary { lock: false } => "",
        _ => " FOR UPDATE OF i",
    };
    let statement = format!(
        "SELECT i.id,i.generation,d.id AS deployment_id,n.manifest,n.id AS installation_id,n.manifest_digest
           FROM provider_records p JOIN instances i ON i.id=p.instance_id
           JOIN deployments d ON d.id=i.deployment_id JOIN installations n ON n.id=d.installation_id
          WHERE p.name=$1 AND p.version=$2 AND p.availability='ready'
            AND p.observed_at>clock_timestamp()-interval '1 millisecond'*
                ((n.manifest->'health'->>'interval_ms')::bigint*(n.manifest->'health'->>'failure_threshold')::bigint
                +(n.manifest->'health'->>'timeout_ms')::bigint)
            AND i.phase='ready' AND d.spec->>'enabled'='true' AND d.archived_at IS NULL
            AND ($3::uuid IS NULL OR d.id=$3) AND ($4::uuid IS NULL OR n.id=$4)
            AND ($5::uuid IS NULL OR (i.id=$5 AND i.generation=$6 AND p.observed_generation=$6))
            AND (NOT $7 OR (i.health='alive' AND p.observed_generation=i.generation))
          ORDER BY i.id LIMIT 1{lock}",
    );
    c.query_opt(
        &statement,
        &[
            &name,
            &version,
            &fixed,
            &installation,
            &instance,
            &generation,
            &fixed_task,
        ],
    )
    .await?
    .ok_or_else(|| Error::unavailable("No healthy fixed provider is available"))
}

fn call_expired() -> Error {
    Error::new(
        "DEADLINE_EXCEEDED",
        "Call admission exceeded its deadline",
        axum::http::StatusCode::GATEWAY_TIMEOUT,
    )
}
