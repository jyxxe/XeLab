//! Bounded kernel-to-plugin migration transport. The durable upgrade supervisor
//! must own the deployment/volume holds, current authority, resource reservation
//! and verified backup before invoking this adapter. It never retries Apply.
use crate::{
    config::{require_host_admin, Config},
    contracts::{digest, strict_json},
    error::{Error, Result},
    rpc::{
        self, data_migration_client::DataMigrationClient,
        plugin_lifecycle_client::PluginLifecycleClient,
    },
    tls,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use tonic::{transport::Channel, Request};
use uuid::Uuid;

const MAX_COUNTER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub id: Uuid,
    pub digest: String,
    pub requires_exclusive_write: bool,
    pub backward_read_compatible: bool,
    pub reversible: bool,
    pub temporary_bytes_required: u64,
}
impl Plan {
    fn validate(&self) -> Result<()> {
        if self.id.is_nil()
            || !valid_digest(&self.digest)
            || self.temporary_bytes_required > MAX_COUNTER
        {
            return Err(Error::conflict("Invalid migration plan response"));
        }
        Ok(())
    }
}
fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}
fn schema(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && !value.chars().any(char::is_control)
}

/// The operation/deployment identities and deadline come from the persisted job,
/// never from a plugin's response. Reconnect after restart with the SAME values.
#[derive(Clone)]
pub struct Binding {
    pub operation: Uuid,
    pub deployment: Uuid,
    pub subject: Uuid,
    pub instance: Uuid,
    pub generation: i64,
    pub expires_unix_ms: i64,
    pub source_schema: String,
    pub target_schema: String,
}
impl Binding {
    fn validate(&self) -> Result<()> {
        if [self.operation, self.deployment, self.subject, self.instance]
            .iter()
            .any(Uuid::is_nil)
            || self.generation <= 0
            || !schema(&self.source_schema)
            || !schema(&self.target_schema)
            || self.source_schema == self.target_schema
        {
            return Err(Error::invalid("Invalid migration binding"));
        }
        Ok(())
    }
}
pub struct Peer {
    binding: Binding,
    channel: Channel,
    deadline: tokio::time::Instant,
}
impl Peer {
    /// Connect only to the supervisor's admitted instance. DNS certificate SAN
    /// and the exact signed Describe bytes both bind the peer to this version.
    pub async fn connect(
        config: &Config,
        binding: Binding,
        address: &str,
        expected_manifest: &Value,
        expected_digest: &str,
    ) -> Result<Self> {
        require_host_admin()?;
        binding.validate()?;
        if !valid_digest(expected_digest)
            || expected_manifest["data_schema_version"] != binding.target_schema
        {
            return Err(Error::invalid("Migration target manifest differs"));
        }
        let remaining = remaining(binding.expires_unix_ms)?.min(Duration::from_secs(3600));
        let deadline = tokio::time::Instant::now() + remaining;
        let channel = tls::channel_with_timeout(
            config,
            binding.instance,
            binding.generation,
            address,
            remaining,
        )
        .await?;
        let peer = Self {
            binding,
            channel,
            deadline,
        };
        peer.describe(expected_manifest, expected_digest).await?;
        Ok(peer)
    }
    async fn describe(&self, expected_manifest: &Value, expected_digest: &str) -> Result<()> {
        let (context, limit) = self.context("describe", Duration::from_secs(10))?;
        let mut request = Request::new(rpc::DescribeRequest {
            context: Some(context),
        });
        request.set_timeout(limit);
        let mut client =
            PluginLifecycleClient::new(self.channel.clone()).max_decoding_message_size(1_100_000);
        let description = tokio::time::timeout(limit, client.describe(request))
            .await
            .map_err(|_| Error::unavailable("Migration Describe deadline expired"))??
            .into_inner();
        if description.manifest_json.len() > 1048576
            || description.manifest_digest != expected_digest
            || digest(&description.manifest_json) != expected_digest
            || strict_json(&description.manifest_json, 1048576)? != *expected_manifest
        {
            return Err(Error::conflict(
                "Migration Describe differs from signed installation",
            ));
        }
        Ok(())
    }
    fn context(&self, phase: &str, cap: Duration) -> Result<(rpc::RpcContext, Duration)> {
        let monotonic = self
            .deadline
            .checked_duration_since(tokio::time::Instant::now())
            .ok_or_else(|| Error::conflict("Migration deadline expired"))?;
        let limit = remaining(self.binding.expires_unix_ms)?
            .min(monotonic)
            .min(cap);
        if limit.is_zero() {
            return Err(Error::conflict("Migration deadline expired"));
        }
        Ok((
            rpc::RpcContext {
                request_id: Uuid::new_v4().to_string(),
                delegation_token: String::new(),
                deadline_unix_ms: (chrono::Utc::now().timestamp_millis()
                    + limit.as_millis() as i64)
                    .min(self.binding.expires_unix_ms),
                idempotency_key: format!("{}:{phase}", self.binding.operation),
                subject_id: self.binding.subject.to_string(),
                actor_instance_id: String::new(),
            },
            limit,
        ))
    }
    fn client(&self) -> DataMigrationClient<Channel> {
        DataMigrationClient::new(self.channel.clone())
            .max_decoding_message_size(65536)
            .max_encoding_message_size(65536)
    }
    pub async fn plan(&self) -> Result<Plan> {
        let (context, limit) = self.context("plan", Duration::from_secs(10))?;
        let mut request = Request::new(rpc::PlanMigrationRequest {
            context: Some(context),
            deployment_id: self.binding.deployment.to_string(),
            source_schema: self.binding.source_schema.clone(),
            target_schema: self.binding.target_schema.clone(),
        });
        request.set_timeout(limit);
        let response = tokio::time::timeout(limit, self.client().plan(request))
            .await
            .map_err(|_| Error::unavailable("Migration Plan deadline expired"))??
            .into_inner();
        let id: Uuid = response
            .plan_id
            .parse()
            .map_err(|_| Error::conflict("Invalid migration plan identity"))?;
        if response.plan_id != id.to_string() {
            return Err(Error::conflict("Noncanonical migration plan identity"));
        }
        let plan = Plan {
            id,
            digest: response.plan_digest,
            requires_exclusive_write: response.requires_exclusive_write,
            backward_read_compatible: response.backward_read_compatible,
            reversible: response.reversible,
            temporary_bytes_required: response.temporary_bytes_required,
        };
        plan.validate()?;
        Ok(plan)
    }
    /// Once handed to transport, even an RPC error has an unknown write outcome.
    /// The caller must persist its applying checkpoint BEFORE this call and use
    /// Verify after ambiguity. No response is a reason to send another Apply.
    pub async fn apply(&self, plan: &Plan, backup: Uuid, fence: u64) -> Result<()> {
        plan.validate()?;
        fence_valid(fence)?;
        if backup.is_nil() {
            return Err(Error::invalid(
                "Migration requires an available backup identity",
            ));
        }
        let (context, limit) = self.context("apply", Duration::from_secs(3600))?;
        let mut request = Request::new(rpc::ApplyMigrationRequest {
            context: Some(context),
            plan_id: plan.id.to_string(),
            plan_digest: plan.digest.clone(),
            backup_id: backup.to_string(),
            volume_fence: fence,
        });
        request.set_timeout(limit);
        let response = tokio::time::timeout(limit, self.client().apply(request)).await;
        match response {
            Ok(Ok(response)) => self
                .result(plan, response.into_inner())
                .map_err(|_| unknown()),
            _ => Err(unknown()),
        }
    }
    pub async fn verify(&self, plan: &Plan, fence: u64) -> Result<()> {
        plan.validate()?;
        fence_valid(fence)?;
        let (context, limit) = self.context("verify", Duration::from_secs(60))?;
        let mut request = Request::new(rpc::VerifyMigrationRequest {
            context: Some(context),
            plan_id: plan.id.to_string(),
            volume_fence: fence,
        });
        request.set_timeout(limit);
        let response = tokio::time::timeout(limit, self.client().verify(request))
            .await
            .map_err(|_| Error::unavailable("Migration Verify deadline expired"))??
            .into_inner();
        self.result(plan, response)
    }
    fn result(&self, plan: &Plan, response: rpc::MigrationResult) -> Result<()> {
        if response.plan_id != plan.id.to_string()
            || !response.succeeded
            || response.error.is_some()
            || response.observed_schema != self.binding.target_schema
        {
            return Err(Error::conflict(
                "Migration data format or verification is unconfirmed",
            ));
        }
        Ok(())
    }
}
fn fence_valid(fence: u64) -> Result<()> {
    if fence == 0 || fence > MAX_COUNTER {
        Err(Error::invalid("Invalid migration volume fence"))
    } else {
        Ok(())
    }
}
fn remaining(expires: i64) -> Result<Duration> {
    let millis = expires
        .checked_sub(chrono::Utc::now().timestamp_millis())
        .filter(|n| *n > 0)
        .ok_or_else(|| Error::conflict("Migration deadline expired"))?;
    Ok(Duration::from_millis(millis as u64))
}
fn unknown() -> Error {
    Error::new(
        "OUTCOME_UNKNOWN",
        "Migration Apply requires Verify before further action",
        axum::http::StatusCode::CONFLICT,
    )
}

#[cfg(test)]
#[path = "migration_rpc_tests.rs"]
mod tests;
