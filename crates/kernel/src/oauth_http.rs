//! OAuth HTTP encoding and trusted native-form handling. Authorization and
//! credential issuance remain in the shared Identity/Authorizer core.
use crate::{
    contracts::{strict_json, validate},
    error::{Error, Result},
};
use axum::http::HeaderMap;
use serde_json::{Map, Value};

const MAXIMUM_FORM_BYTES: usize = 16 * 1024;

pub(crate) struct ConsentSubmission {
    pub decision: Value,
    pub csrf_token: String,
    pub idempotency_key: String,
}

fn one_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>> {
    if headers.get_all(name).iter().count() > 1 {
        return Err(Error::invalid("Repeated authentication header"));
    }
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| Error::invalid("Invalid authentication header"))
        })
        .transpose()
}

fn media_type(headers: &HeaderMap) -> Result<&str> {
    if headers.contains_key("content-encoding") {
        return Err(Error::invalid(
            "Encoded authentication bodies are not accepted",
        ));
    }
    let value = one_header(headers, "content-type")?
        .ok_or_else(|| Error::invalid("Authentication Content-Type is required"))?;
    let mut parts = value.split(';');
    let media = parts.next().unwrap_or("").trim();
    let mut charset = false;
    for parameter in parts {
        let Some((name, value)) = parameter.trim().split_once('=') else {
            return Err(Error::invalid("Invalid authentication Content-Type"));
        };
        if charset
            || !name.trim().eq_ignore_ascii_case("charset")
            || !value.trim().eq_ignore_ascii_case("utf-8")
        {
            return Err(Error::invalid("Unsupported authentication body encoding"));
        }
        charset = true;
    }
    Ok(media)
}

fn form(bytes: &[u8]) -> Result<Map<String, Value>> {
    crate::contracts::strict_urlencoded(bytes, MAXIMUM_FORM_BYTES, 16)
}

pub(crate) fn token_request(headers: &HeaderMap, bytes: &[u8]) -> Result<Value> {
    if !media_type(headers)?.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
        return Err(Error::invalid(
            "OAuth token endpoint requires a URL encoded form",
        ));
    }
    let value = Value::Object(form(bytes)?);
    validate("OAuthTokenRequest", &value)?;
    Ok(value)
}

fn metadata(
    headers: &HeaderMap,
    fields: &mut Map<String, Value>,
    name: &str,
    minimum: usize,
    maximum: usize,
) -> Result<String> {
    let header = one_header(headers, name)?;
    let field = fields.remove(name);
    let value = match (header, field) {
        (Some(value), None) => value.to_owned(),
        (None, Some(Value::String(value))) => value,
        _ => {
            return Err(Error::invalid(
                "Authentication metadata must have one source",
            ))
        }
    };
    if !(minimum..=maximum).contains(&value.chars().count()) {
        return Err(Error::invalid("Authentication metadata size is invalid"));
    }
    Ok(value)
}

pub(crate) fn consent_request(headers: &HeaderMap, bytes: &[u8]) -> Result<ConsentSubmission> {
    let media = media_type(headers)?;
    let native = media.eq_ignore_ascii_case("application/x-www-form-urlencoded");
    let mut fields = if native {
        let fields = form(bytes)?;
        validate("ConsentForm", &Value::Object(fields.clone()))?;
        fields
    } else if media.eq_ignore_ascii_case("application/json") {
        let value = strict_json(bytes, MAXIMUM_FORM_BYTES)?;
        validate("ConsentDecision", &value)?;
        value
            .as_object()
            .cloned()
            .ok_or_else(|| Error::invalid("Consent decision must be an object"))?
    } else {
        return Err(Error::invalid("Unsupported consent Content-Type"));
    };
    let csrf_token = metadata(headers, &mut fields, "X-CSRF-Token", 43, 128)?;
    let idempotency_key = metadata(headers, &mut fields, "Idempotency-Key", 1, 128)?;
    if native {
        let approved = match fields.remove("approved") {
            Some(Value::String(value)) if value == "true" => true,
            Some(Value::String(value)) if value == "false" => false,
            _ => return Err(Error::invalid("Consent approved must be true or false")),
        };
        fields.insert("approved".into(), Value::Bool(approved));
    }
    let decision = Value::Object(fields);
    validate("ConsentDecision", &decision)?;
    Ok(ConsentSubmission {
        decision,
        csrf_token,
        idempotency_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(media: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_str(media).unwrap());
        headers
    }

    #[test]
    fn token_form_rejects_duplicates_aliases_bad_utf8_and_mixed_grants() {
        let headers = headers("application/x-www-form-urlencoded; charset=UTF-8");
        let valid = b"grant_type=refresh_token&client_id=client&refresh_token=credential";
        assert_eq!(
            token_request(&headers, valid).unwrap()["grant_type"],
            "refresh_token"
        );
        for body in [
            b"grant_type=refresh_token&%67rant_type=authorization_code&client_id=x&refresh_token=x"
                .as_slice(),
            b"grant_type=refresh_token&client_id=x&refresh_token=%ff",
            b"grant_type=refresh_token&client_id=x&refresh_token=%G1",
            b"grant_type=refresh_token&client_id=x&refresh_token=%",
            b"grant_type=refresh_token&client_id=x&refresh_token=x&code=also-code",
            b"grant_type=refresh_token&client_id=x&refresh_token=x&account_id=forged",
            b"grant_type=refresh_token&client_id=x&refresh_token=x&",
        ] {
            assert!(token_request(&headers, body).is_err(), "{body:?}");
        }
        assert!(form(&vec![b'a'; MAXIMUM_FORM_BYTES + 1]).is_err());
        assert!(token_request(&self::headers("application/json"), valid).is_err());
    }

    #[test]
    fn native_consent_metadata_is_exclusive_and_decision_stays_unchanged() {
        let mut headers = headers("application/x-www-form-urlencoded");
        let csrf = "a".repeat(43);
        let body = format!("authorization_request_id=00000000-0000-4000-8000-000000000001&approved=false&X-CSRF-Token={csrf}&Idempotency-Key=one-time");
        let parsed = consent_request(&headers, body.as_bytes()).unwrap();
        assert_eq!(parsed.decision["approved"], false);
        assert_eq!(parsed.decision.as_object().unwrap().len(), 2);
        assert_eq!(parsed.csrf_token, csrf);
        assert_eq!(parsed.idempotency_key, "one-time");
        headers.insert("x-csrf-token", HeaderValue::from_str(&csrf).unwrap());
        assert!(consent_request(&headers, body.as_bytes()).is_err());
        headers.remove("x-csrf-token");
        headers.insert("idempotency-key", HeaderValue::from_static("different"));
        assert!(consent_request(&headers, body.as_bytes()).is_err());
        headers.remove("idempotency-key");
        for value in ["1", "yes", "True", "false&approved=true"] {
            assert!(consent_request(
                &headers,
                body.replace("approved=false", &format!("approved={value}"))
                    .as_bytes()
            )
            .is_err());
        }
    }

    #[test]
    fn json_consent_does_not_accept_form_metadata_or_duplicate_headers() {
        let mut headers = headers("application/json");
        headers.insert(
            "x-csrf-token",
            HeaderValue::from_str(&"b".repeat(43)).unwrap(),
        );
        headers.insert("idempotency-key", HeaderValue::from_static("request-one"));
        let value = serde_json::json!({"authorization_request_id":"00000000-0000-4000-8000-000000000001","approved":true});
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(consent_request(&headers, &bytes).unwrap().decision["approved"] == true);
        let mut with_metadata = value;
        with_metadata["X-CSRF-Token"] = Value::String("b".repeat(43));
        assert!(consent_request(&headers, &serde_json::to_vec(&with_metadata).unwrap()).is_err());
        headers.append("idempotency-key", HeaderValue::from_static("request-one"));
        assert!(consent_request(&headers, &bytes).is_err());
    }
}
