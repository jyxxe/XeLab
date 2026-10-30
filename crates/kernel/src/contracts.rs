use crate::error::{Error, Result};
use serde::{
    de::{self, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

pub const OPENAPI: &str = include_str!("../../../docs/contracts/openapi.json");

pub fn openapi() -> &'static Value {
    static DOC: OnceLock<Value> = OnceLock::new();
    DOC.get_or_init(|| serde_json::from_str(OPENAPI).expect("checked-in OpenAPI must parse"))
}

pub fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub fn validate(name: &str, input: &Value) -> Result<()> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<jsonschema::JSONSchema>>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| Error::unavailable("Validator unavailable"))?;
    let validator = if let Some(v) = cache.get(name) {
        v.clone()
    } else {
        if openapi()["components"]["schemas"].get(name).is_none() {
            return Err(Error::invalid("Unknown model"));
        }
        let doc = json!({"$ref":format!("#/components/schemas/{name}"), "components":openapi()["components"]});
        let v = Arc::new(
            jsonschema::JSONSchema::options()
                .with_draft(jsonschema::Draft::Draft202012)
                .should_validate_formats(true)
                .compile(&doc)
                .map_err(|_| Error::invalid("Invalid contract"))?,
        );
        cache.insert(name.to_owned(), v.clone());
        v
    };
    drop(cache);
    if validator.is_valid(input) {
        Ok(())
    } else {
        Err(Error::invalid("Value does not match the existing contract"))
    }
}

pub fn validate_business(schema: &Value, input: &Value) -> Result<()> {
    reject_refs(schema)?;
    let compiled = jsonschema::JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .should_validate_formats(true)
        .compile(schema)
        .map_err(|_| Error::invalid("Invalid business schema"))?;
    if compiled.is_valid(input) {
        Ok(())
    } else {
        Err(Error::invalid("Business value does not match its schema"))
    }
}

// M1 accepts self-contained schemas only. It never resolves network or host-file refs.
pub fn reject_refs(value: &Value) -> Result<()> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if matches!(
                    key.as_str(),
                    "$ref" | "$dynamicRef" | "$recursiveRef" | "$id"
                ) {
                    return Err(Error::invalid(
                        "Referenced schemas are not supported by this M1 profile",
                    ));
                }
                reject_refs(value)?;
            }
        }
        Value::Array(a) => {
            for v in a {
                reject_refs(v)?;
            }
        }
        _ => (),
    }
    Ok(())
}

pub fn strict_json(bytes: &[u8], maximum: usize) -> Result<Value> {
    if bytes.len() > maximum {
        return Err(Error::new(
            "PAYLOAD_TOO_LARGE",
            "JSON exceeds its byte limit",
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
        ));
    }
    let mut d = serde_json::Deserializer::from_slice(bytes);
    let value = StrictValue::deserialize(&mut d)?;
    d.end()?;
    Ok(value.0)
}

struct StrictValue(Value);
impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = StrictValue;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("strict JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|x| StrictValue(Value::Number(x)))
                    .ok_or_else(|| E::custom("nonfinite number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut a = Vec::new();
                while let Some(v) = seq.next_element::<StrictValue>()? {
                    a.push(v.0);
                }
                Ok(StrictValue(Value::Array(a)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut a = Map::new();
                while let Some(k) = map.next_key::<String>()? {
                    if a.contains_key(&k) {
                        return Err(de::Error::custom("duplicate JSON member"));
                    }
                    a.insert(k, map.next_value::<StrictValue>()?.0);
                }
                Ok(StrictValue(Value::Object(a)))
            }
        }
        d.deserialize_any(V)
    }
}

pub fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| Error::invalid("Required string is missing"))
}
pub fn id(v: &Value, key: &str) -> Result<uuid::Uuid> {
    string(v, key)?
        .parse()
        .map_err(|_| Error::invalid("Invalid resource UUID"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_and_trailing_json_are_rejected() {
        for input in [
            br#"{"a":1,"a":2}"#.as_slice(),
            br#"{"a":{"x":0,"x":1}}"#,
            b"{} {}",
            b"NaN",
        ] {
            assert!(strict_json(input, 1024).is_err());
        }
    }
    #[test]
    fn schemas_are_offline_and_strict() {
        assert!(validate("NodeConfig",&json!({"kernel_cpu_millis":3999,"kernel_memory_bytes":17179869184u64,"system_cpu_millis":1000,"system_memory_bytes":1048576,"shutdown_grace_ms":30000})).is_err());
        assert!(
            reject_refs(&json!({"properties":{"secret":{"$ref":"file:///etc/passwd"}}})).is_err()
        );
        assert!(validate(
            "HealthStatus",
            &json!({"status":"ok","spec_version":"2.8.0","extra":true})
        )
        .is_err());
    }
}
