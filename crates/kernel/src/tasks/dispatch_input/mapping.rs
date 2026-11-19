use crate::{
    contracts::string,
    error::{Error, Result},
};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    io::{self, Write},
};
use tokio_postgres::Transaction;
use uuid::Uuid;

// CapabilityContract permits at most 4 MiB of compact business input.
// Account for JSON escaping, punctuation and each repeated expansion.
const MAX_INPUT: usize = 4 * 1024 * 1024;
// Committed outputs can be 16 MiB compact. PostgreSQL's wire JSON adds spaces.
// Read one referenced source at a time and release it after all its pointers.
const MAX_SOURCE: i32 = 32 * 1024 * 1024;

struct Remaining(usize);

impl Write for Remaining {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(buf.len())
            .ok_or_else(|| io::Error::other("Mapped input exceeds the Broker size limit"))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Input {
    fields: Map<String, Value>,
    budget: Remaining,
}

impl Input {
    fn new() -> Self {
        Self {
            fields: Map::new(),
            budget: Remaining(MAX_INPUT - 2),
        } // braces
    }

    fn insert(&mut self, field: &str, value: &Value) -> Result<()> {
        let separators = if self.fields.is_empty() { 1 } else { 2 }; // colon, optional comma
        self.budget.0 = self
            .budget
            .0
            .checked_sub(separators)
            .ok_or_else(Error::exhausted)?;
        serde_json::to_writer(&mut self.budget, field)
            .and_then(|_| serde_json::to_writer(&mut self.budget, value))
            .map_err(|_| Error::exhausted())?;
        self.fields.insert(field.to_owned(), value.clone());
        Ok(())
    }
}

pub(super) async fn resolve(tx: &Transaction<'_>, task: Uuid, mapping: &Value) -> Result<Value> {
    let mut input = Input::new();
    let mut sources: BTreeMap<Option<&str>, Vec<(&str, &str)>> = BTreeMap::new();
    // The immutable spec was validated, including oneOf exclusivity for each leaf.
    for (field, leaf) in mapping.as_object().unwrap() {
        if let Some(literal) = leaf.get("literal") {
            input.insert(field, literal)?;
            continue;
        }
        let (source, pointer) = if let Some(output) = leaf.get("step_output") {
            (Some(string(output, "step")?), string(output, "pointer")?)
        } else {
            (None, string(leaf, "task_input")?)
        };
        sources.entry(source).or_default().push((field, pointer));
    }

    for (source, pointers) in sources {
        let row = match source {
            None => {
                tx.query_opt(
                    "SELECT CASE WHEN octet_length(input::text)<=$2 THEN input::text END
                   FROM tasks WHERE id=$1",
                    &[&task, &MAX_SOURCE],
                )
                .await?
            }
            Some(key) => {
                tx.query_opt(
                    "SELECT CASE WHEN octet_length(output::text)<=$3 THEN output::text END
                   FROM task_steps WHERE task_id=$1 AND step_key=$2 AND state='succeeded'
                    AND output IS NOT NULL",
                    &[&task, &key, &MAX_SOURCE],
                )
                .await?
            }
        }
        .ok_or_else(|| Error::invalid("Workflow mapping source is missing or incomplete"))?;
        let raw: &str = row
            .try_get::<_, Option<&str>>(0)?
            .ok_or_else(Error::exhausted)?;
        let value: Value = serde_json::from_str(raw)?;
        for (field, pointer) in pointers {
            // Value::pointer distinguishes object keys from RFC 6901 array indices.
            let selected = value
                .pointer(pointer)
                .ok_or_else(|| Error::invalid("Workflow mapping pointer does not exist"))?;
            input.insert(field, selected)?;
        }
    }
    Ok(Value::Object(input.fields))
}
