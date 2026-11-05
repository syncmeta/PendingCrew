//! Preserve the V2 intent boundary before Request.params loses duplicate keys.
//! Old routes retain their existing parser and authorization ordering.
use crewd_proto::{Request, agent::methods, managed_tables_v2};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::{collections::HashSet, fmt};

pub(crate) fn enforced(request: &Request, attached: bool) -> bool {
    if !attached {
        return managed_tables_v2::METHODS.contains(&request.method.as_str());
    }
    request.method == methods::CALL
        && request
            .params
            .get("tool")
            .and_then(serde_json::Value::as_str)
            .is_some_and(is_v2_tool)
}

/// Both the authenticated socket and the MCP helper use this exact tool set.
pub fn is_v2_tool(tool: &str) -> bool {
    matches!(
        tool,
        "managed_table_v2_get"
            | "managed_tables_v2_list"
            | "managed_row_v2_get"
            | "managed_rows_v2_list"
            | "managed_table_v2_submit"
            | "managed_table_v2_request_get"
            | "managed_link_resolve_target"
            | "managed_links_list"
            | "managed_link_submit"
            | "managed_link_request_get"
    )
}

/// Pure JSON validation; this never grants authority or normalizes input.
pub fn unique_fields(raw: &str) -> Result<(), serde_json::Error> {
    serde_json::from_str::<UniqueFields>(raw).map(|_| ())
}

struct UniqueFields;
impl<'de> Deserialize<'de> for UniqueFields {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Check;
        impl<'de> Visitor<'de> for Check {
            type Value = UniqueFields;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("JSON without duplicate object fields")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut keys = HashSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !keys.insert(key) {
                        return Err(de::Error::custom("duplicate object field"));
                    }
                    map.next_value::<UniqueFields>()?;
                }
                Ok(UniqueFields)
            }
            fn visit_seq<S: SeqAccess<'de>>(
                self,
                mut sequence: S,
            ) -> Result<Self::Value, S::Error> {
                while sequence.next_element::<UniqueFields>()?.is_some() {}
                Ok(UniqueFields)
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueFields)
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(UniqueFields)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(UniqueFields)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(UniqueFields)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(UniqueFields)
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(UniqueFields)
            }
            fn visit_string<E: de::Error>(self, _: String) -> Result<Self::Value, E> {
                Ok(UniqueFields)
            }
        }
        deserializer.deserialize_any(Check)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nested_duplicates_and_escaped_aliases_are_rejected_before_value_normalization() {
        for raw in [
            r#"{"operation":{"kind":"keep","kind":"replaceLinks"}}"#,
            r#"{"patches":[{"value":null,"value":1}]}"#,
            r#"{"x":0,"\u0078":1}"#,
        ] {
            assert!(serde_json::from_str::<serde_json::Value>(raw).is_ok());
            assert!(unique_fields(raw).is_err(), "{raw}");
        }
        assert!(
            unique_fields(r#"{"a":[{"x":0},{"x":1}],"b":-0.0,"c":"{\"x\":1,\"x\":2}"}"#).is_ok()
        );
        assert!(
            unique_fields("{\"é\":1,\"e\\u0301\":2}").is_ok(),
            "do not normalize UTF-8 keys"
        );
    }

    #[test]
    fn strict_input_does_not_bypass_attachment_or_change_old_routes() {
        let request = |method: &str, tool: Option<&str>| Request {
            id: 1,
            method: method.into(),
            params: json!({"tool":tool}),
        };
        for method in managed_tables_v2::METHODS {
            assert!(enforced(&request(method, None), false));
            assert!(
                !enforced(&request(method, None), true),
                "attached Human route must refuse first"
            );
        }
        for tool in [
            "managed_table_v2_get",
            "managed_tables_v2_list",
            "managed_row_v2_get",
            "managed_rows_v2_list",
            "managed_table_v2_submit",
            "managed_table_v2_request_get",
            "managed_link_resolve_target",
            "managed_links_list",
            "managed_link_submit",
            "managed_link_request_get",
        ] {
            assert!(enforced(&request(methods::CALL, Some(tool)), true));
            assert!(
                !enforced(&request(methods::CALL, Some(tool)), false),
                "unattached Agent call must refuse first"
            );
        }
        assert!(!enforced(&request("managedObject.submit", None), false));
        assert!(!enforced(
            &request(methods::CALL, Some("managed_object_submit")),
            true
        ));
    }
}
