//! Lossless field values at the transport boundary. HTTP parsing normalizes
//! names and groups repeated names; this is not a capture of wire casing/order.
use base64::Engine;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawHeader {
    pub name: Vec<u8>,
    pub value: Vec<u8>,
}

impl Serialize for RawHeader {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let base64 = base64::engine::general_purpose::STANDARD;
        let mut field = serializer.serialize_struct("RawHeader", 2)?;
        field.serialize_field("nameBase64", &base64.encode(&self.name))?;
        field.serialize_field("valueBase64", &base64.encode(&self.value))?;
        field.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderCapture {
    pub capture_stage: &'static str,
    pub encoding: &'static str,
    /// Transport iteration order, preserving every value for repeated names.
    /// HTTP wire casing, cross-name order and framing are not represented.
    pub fields: Vec<RawHeader>,
}

impl HeaderCapture {
    pub(crate) fn from_headers(stage: &'static str, headers: &http::HeaderMap) -> Self {
        Self {
            capture_stage: stage,
            encoding: "base64",
            fields: headers.iter().map(|(name, value)| RawHeader {
                name: name.as_str().as_bytes().to_vec(),
                value: value.as_bytes().to_vec(),
            }).collect(),
        }
    }

    /// Compatibility view only. Names with any non-UTF-8 value have no string projection;
    /// every original value remains in `fields`, including repeated Set-Cookie.
    pub fn text_headers(&self) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        let mut non_text_names = std::collections::HashSet::new();
        for field in &self.fields {
            let Ok(name) = std::str::from_utf8(&field.name) else { continue; };
            match std::str::from_utf8(&field.value) {
                Ok(value) if !non_text_names.contains(name) => {
                    crate::client::merge_response_header(&mut headers, name.to_owned(), value.to_owned());
                }
                Err(_) => {
                    // A partial text projection could turn an invalid repeated
                    // CORS/security field into a valid singleton.
                    headers.remove(name);
                    non_text_names.insert(name);
                }
                _ => {}
            }
        }
        headers
    }
}
