//! Output provenance for history, independent of provider and Skill enablement.
use super::super::{claim_authority, commit_effects};
use crate::{
    contracts::{id, string, unsigned, validate_business},
    error::{Error, Result},
};
use serde_json::Value;
use tokio_postgres::Transaction;
use uuid::Uuid;

pub(super) struct Historical<'a> {
    binding: &'a Value,
    installation: Option<Uuid>,
    skill: Option<Uuid>,
}

fn unavailable() -> Error {
    Error::conflict("The original task output binding cannot be verified")
}

pub(super) async fn historical<'a>(
    tx: &Transaction<'_>,
    workflow: Uuid,
    admission: Option<&'a Value>,
    key: &str,
    deployment: Option<Uuid>,
) -> Result<Historical<'a>> {
    let admission = admission
        .filter(|value| value["version"] == 1)
        .ok_or_else(unavailable)?;
    let record = exact_step(&admission["steps"], "step_key", key)?;
    let spec: Value = tx
        .query_one(
            "SELECT spec FROM workflows WHERE id=$1 FOR SHARE",
            &[&workflow],
        )
        .await?
        .try_get(0)?;
    let step = exact_step(&spec["steps"], "id", key)?;
    let skill = step
        .get("skill_id")
        .map(|_| id(step, "skill_id"))
        .transpose()?;
    let installation = serde_json::from_value::<Option<Uuid>>(
        record
            .get("installation_id")
            .ok_or_else(unavailable)?
            .clone(),
    )
    .map_err(|_| unavailable())?;
    if installation.is_some() != deployment.is_some() {
        return Err(unavailable());
    }
    claim_authority::check_deployment(step, record, deployment).map_err(|_| unavailable())?;
    let binding = &record["binding"];
    claim_authority::validate_binding(binding, step, skill, deployment).map_err(|error| {
        if error.code == "PERMISSION_DENIED" {
            unavailable()
        } else {
            error
        }
    })?;
    Ok(Historical {
        binding,
        installation,
        skill,
    })
}

fn exact_step<'a>(steps: &'a Value, field: &str, key: &str) -> Result<&'a Value> {
    let mut matches = steps
        .as_array()
        .ok_or_else(unavailable)?
        .iter()
        .filter(|step| step[field] == key);
    let found = matches.next().ok_or_else(unavailable)?;
    if matches.next().is_some() {
        return Err(unavailable());
    }
    Ok(found)
}

impl Historical<'_> {
    pub(super) async fn validate_output(&self, tx: &Transaction<'_>, output: &Value) -> Result<()> {
        // The deployment can now point at a different installation. Select the
        // exact manifest frozen in the admission, even after archival.
        let manifest: Option<Value> = if let Some(installation) = self.installation {
            Some(
                tx.query_opt(
                    "SELECT manifest FROM installations WHERE id=$1 FOR SHARE",
                    &[&installation],
                )
                .await?
                .ok_or_else(unavailable)?
                .try_get(0)?,
            )
        } else {
            None
        };
        let contract = &self.binding["contract"];
        if let Some(skill) = self.skill {
            let installation = self.installation.ok_or_else(unavailable)?;
            let row = tx.query_opt(
                "SELECT definition,definition_digest FROM skills WHERE id=$1 AND installation_id=$2
                  AND source_kind='oci' AND micro_version_id IS NULL AND micro_deployment_id IS NULL
                  AND extension_version_id IS NULL AND extension_deployment_id IS NULL
                  AND extension_host_chain_digest IS NULL FOR SHARE",
                &[&skill, &installation],
            ).await?.ok_or_else(unavailable)?;
            let definition: Value = row.try_get("definition")?;
            let digest: &str = row.try_get("definition_digest")?;
            let kind = match definition["target"]["kind"].as_str() {
                Some("capability") => "skill_capability",
                Some("http") => "skill_http",
                _ => return Err(unavailable()),
            };
            if self.binding["kind"] != kind
                || self.binding["definition_digest"] != digest
                || crate::skills::definition_digest(&definition)? != digest
            {
                return Err(unavailable());
            }
            crate::skills::candidate_binding(
                tx,
                installation,
                manifest.as_ref().ok_or_else(unavailable)?,
                &definition,
            )
            .await?;

            if kind == "skill_http" {
                let registry = crate::http_contracts::load(tx, installation).await?;
                let operation = registry.bind_skill(&definition)?;
                if self.binding["operation_id"] != operation.id
                    || self.binding["http_contract_digest"] != operation.contract_digest
                    || self.binding["method"] != operation.method.as_str()
                    || self.binding["path"] != operation.path
                {
                    return Err(unavailable());
                }
                let max = if definition["stream_item_schema"].is_null() {
                    crate::plugin_http::MAXIMUM_RESPONSE_BYTES
                } else {
                    1024 * 1024
                };
                commit_effects::check_output_bytes(output, max as u64)?;
                return validate_business(&definition["output_schema"], output);
            }
            if definition["target"]["contract"] != *contract {
                return Err(unavailable());
            }
        }

        let registered: Value = tx
            .query_opt(
                "SELECT spec FROM capability_contracts
                 WHERE name=$1 AND version=$2 AND schema_digest=$3 FOR SHARE",
                &[
                    &string(contract, "name")?,
                    &string(contract, "version")?,
                    &string(contract, "schema_digest")?,
                ],
            )
            .await?
            .ok_or_else(unavailable)?
            .try_get(0)?;
        let capability = match &manifest {
            Some(manifest) => manifest["capabilities"]
                .as_array()
                .and_then(|list| list.iter().find(|value| value["contract"] == *contract))
                .ok_or_else(unavailable)?,
            None => &registered,
        };
        commit_effects::check_output_bytes(output, unsigned(&capability["maximum_output_bytes"])?)?;
        validate_business(&capability["output_schema"], output)
    }
}
