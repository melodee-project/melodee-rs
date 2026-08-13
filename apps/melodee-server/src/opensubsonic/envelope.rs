//! OpenSubsonic envelope and format negotiation (PRD-API-OS-002).
//!
//! The frozen contract: JSON `{subsonic-response: {...}}`, XML default with
//! hardcoded lowercase `type="melodee"`, JSONP `callback(json)` with
//! `application/javascript`, and `f=jsonp` without a callback falling back to
//! XML. JSON compares structurally; XML and JSONP compare byte-for-byte after
//! volatile masks, so the XML writer below is deliberately mechanical.

use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::Value;

use super::params::Params;

pub const API_VERSION: &str = "1.16.1";
pub const SERVER_TYPE_JSON: &str = "Melodee";
pub const SERVER_TYPE_XML: &str = "melodee";
pub const XMLNS: &str = "https://subsonic.org/restapi";

/// Negotiated response format. `f=jsonp` without `callback` is XML, matching
/// the captured `ApiRequest.IsXmlRequest` behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Format {
    Xml,
    Json,
    Jsonp(String),
}

pub fn negotiate(params: &Params) -> Format {
    match params.get("f") {
        Some("json") => Format::Json,
        Some("jsonp") => match params.get("callback") {
            Some(callback) if !callback.is_empty() => Format::Jsonp(callback.to_string()),
            _ => Format::Xml,
        },
        _ => Format::Xml,
    }
}

/// What an endpoint produced: ordered payload entries that sit inside the
/// envelope next to the fixed attributes.
#[derive(Debug, Clone)]
pub struct OsReply {
    /// Payload object entries in wire order (empty for bare ok/error replies).
    pub payload: Value,
    /// Captured XML quirks that the generic writer cannot express (the
    /// double-wrapped license element, the malformed extensions elements).
    /// When set, this replaces the generically written payload children.
    pub xml_children_override: Option<String>,
    /// Value for the `x-total-count` response header; list endpoints report
    /// their total, everything else sends the captured `0`.
    pub total_count: i64,
}

impl OsReply {
    pub fn ok(payload: Value) -> Self {
        Self {
            payload,
            xml_children_override: None,
            total_count: 0,
        }
    }

    pub fn error(code: i64, message: &str) -> Self {
        Self::ok(serde_json::json!({
            "error": {"code": code, "message": message}
        }))
    }

    #[must_use]
    pub fn with_xml_children_override(mut self, xml: String) -> Self {
        self.xml_children_override = Some(xml);
        self
    }

    #[must_use]
    pub const fn with_total_count(mut self, total_count: i64) -> Self {
        self.total_count = total_count;
        self
    }

    fn is_error(&self) -> bool {
        self.payload.get("error").is_some()
    }

    fn json_envelope(&self, server_version: &str) -> Value {
        let mut envelope = serde_json::Map::new();
        envelope.insert(
            "status".to_string(),
            Value::from(if self.is_error() { "error" } else { "ok" }),
        );
        envelope.insert("version".to_string(), Value::from(API_VERSION));
        envelope.insert("type".to_string(), Value::from(SERVER_TYPE_JSON));
        envelope.insert("serverVersion".to_string(), Value::from(server_version));
        envelope.insert("openSubsonic".to_string(), Value::from(true));
        if let Value::Object(entries) = &self.payload {
            for (key, value) in entries {
                envelope.insert(key.clone(), value.clone());
            }
        }
        let mut root = serde_json::Map::new();
        root.insert("subsonic-response".to_string(), Value::Object(envelope));
        Value::Object(root)
    }

    fn xml_envelope(&self, server_version: &str) -> String {
        let mut out = String::with_capacity(256);
        out.push_str("<subsonic-response xmlns=\"");
        out.push_str(XMLNS);
        out.push_str("\" status=\"");
        out.push_str(if self.is_error() { "error" } else { "ok" });
        out.push_str("\" type=\"");
        out.push_str(SERVER_TYPE_XML);
        out.push_str("\" version=\"");
        out.push_str(API_VERSION);
        out.push_str("\" serverVersion=\"");
        xml_escape_into(server_version, &mut out);
        out.push_str("\" openSubsonic=\"true\">");
        if let Some(xml) = &self.xml_children_override {
            out.push_str(xml);
        } else if let Value::Object(entries) = &self.payload {
            for (key, value) in entries {
                write_child(&mut out, key, value);
            }
        }
        out.push_str("</subsonic-response>");
        out
    }

    /// Renders into the negotiated format. OpenSubsonic answers application
    /// errors with HTTP 200 and an error envelope; the HTTP status only moves
    /// for endpoint classes captured with one (podcast auth 403, stream 404).
    pub fn respond(
        self,
        format: &Format,
        server_version: &str,
        http_status: StatusCode,
    ) -> Response {
        let (content_type, body) = match format {
            // The wire format is compact JSON; the pretty-printed golden
            // files are a capture-sanitization artifact (their recorded
            // content-length matches compact bytes).
            Format::Json => (
                "application/json; charset=utf-8",
                serde_json::to_string(&self.json_envelope(server_version))
                    .unwrap_or_else(|_| "{}".to_string()),
            ),
            Format::Jsonp(callback) => (
                "application/javascript; charset=utf-8",
                format!(
                    "{callback}({})",
                    serde_json::to_string(&self.json_envelope(server_version))
                        .unwrap_or_else(|_| "{}".to_string())
                ),
            ),
            Format::Xml => ("application/xml", self.xml_envelope(server_version)),
        };
        let mut response = (http_status, Body::from(body)).into_response();
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(match format {
                Format::Json => "application/json; charset=utf-8",
                Format::Jsonp(_) => "application/javascript; charset=utf-8",
                Format::Xml => "application/xml",
            }),
        );
        if let Ok(value) = HeaderValue::from_str(&self.total_count.to_string()) {
            headers.insert(header::HeaderName::from_static("x-total-count"), value);
        }
        let _ = content_type;
        response
    }
}

fn xml_escape_into(value: &str, out: &mut String) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(if *flag { "true" } else { "false" }.to_string()),
        _ => None,
    }
}

/// Subsonic XML convention: scalar fields become attributes, nested objects
/// and arrays become child elements, arrays repeat the key. Elements with no
/// children self-close; elements with children use explicit close tags.
fn write_child(out: &mut String, key: &str, value: &Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                write_child(out, key, item);
            }
        }
        Value::Object(entries) => {
            out.push('<');
            out.push_str(key);
            let mut children: Vec<(&String, &Value)> = Vec::new();
            for (child_key, child_value) in entries {
                if let Some(scalar) = scalar_text(child_value) {
                    out.push(' ');
                    out.push_str(child_key);
                    out.push_str("=\"");
                    xml_escape_into(&scalar, out);
                    out.push('"');
                } else if !child_value.is_null() {
                    children.push((child_key, child_value));
                }
            }
            if children.is_empty() {
                out.push_str("/>");
            } else {
                out.push('>');
                for (child_key, child_value) in children {
                    write_child(out, child_key, child_value);
                }
                out.push_str("</");
                out.push_str(key);
                out.push('>');
            }
        }
        Value::Null => {}
        scalar => {
            if let Some(text) = scalar_text(scalar) {
                out.push('<');
                out.push_str(key);
                out.push('>');
                xml_escape_into(&text, out);
                out.push_str("</");
                out.push_str(key);
                out.push('>');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn params(pairs: &[(&str, &str)]) -> Params {
        let mut params = Params::default();
        for (key, value) in pairs {
            params.add(key, value);
        }
        params
    }

    #[test]
    fn format_negotiation_matches_the_captured_contract() {
        assert_eq!(negotiate(&params(&[])), Format::Xml);
        assert_eq!(negotiate(&params(&[("f", "xml")])), Format::Xml);
        assert_eq!(negotiate(&params(&[("f", "json")])), Format::Json);
        assert_eq!(negotiate(&params(&[("f", "jsonp")])), Format::Xml);
        assert_eq!(
            negotiate(&params(&[("f", "jsonp"), ("callback", "cb")])),
            Format::Jsonp("cb".to_string())
        );
    }

    #[test]
    fn bare_ok_xml_matches_the_captured_ping_bytes() {
        let xml = OsReply::ok(serde_json::json!({})).xml_envelope("9.9.9");
        assert_eq!(
            xml,
            "<subsonic-response xmlns=\"https://subsonic.org/restapi\" status=\"ok\" type=\"melodee\" version=\"1.16.1\" serverVersion=\"9.9.9\" openSubsonic=\"true\"></subsonic-response>"
        );
    }

    #[test]
    fn error_xml_uses_self_closing_error_element() {
        let xml = OsReply::error(40, "Wrong username or password.").xml_envelope("9.9.9");
        assert_eq!(
            xml,
            "<subsonic-response xmlns=\"https://subsonic.org/restapi\" status=\"error\" type=\"melodee\" version=\"1.16.1\" serverVersion=\"9.9.9\" openSubsonic=\"true\"><error code=\"40\" message=\"Wrong username or password.\"/></subsonic-response>"
        );
    }

    #[test]
    fn music_folders_xml_matches_the_captured_shape() {
        let reply = OsReply::ok(serde_json::json!({
            "musicFolders": {
                "musicFolder": [
                    {"id": "library_1", "name": "One"},
                    {"id": "library_2", "name": "Two"}
                ]
            }
        }));
        let xml = reply.xml_envelope("9.9.9");
        assert_eq!(
            xml,
            "<subsonic-response xmlns=\"https://subsonic.org/restapi\" status=\"ok\" type=\"melodee\" version=\"1.16.1\" serverVersion=\"9.9.9\" openSubsonic=\"true\"><musicFolders><musicFolder id=\"library_1\" name=\"One\"/><musicFolder id=\"library_2\" name=\"Two\"/></musicFolders></subsonic-response>"
        );
    }

    #[test]
    fn json_envelope_keeps_the_captured_key_order() {
        let json = OsReply::ok(serde_json::json!({})).json_envelope("9.9.9");
        let rendered = serde_json::to_string(&json).expect("serialize");
        assert_eq!(
            rendered,
            "{\"subsonic-response\":{\"status\":\"ok\",\"version\":\"1.16.1\",\"type\":\"Melodee\",\"serverVersion\":\"9.9.9\",\"openSubsonic\":true}}"
        );
    }
}
