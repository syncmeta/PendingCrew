//! Version selection for the existing six MCP table tools. This is not authority.
use crate::managed_tables_v3 as v3;
use schemars::JsonSchema;
use serde_json::Value;

/// Keep actual table declarations within the existing 32 KiB wire bound.
pub const TOOL_SCHEMA_MAX_BYTES: usize = 32_768;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContractVersion {
    V2,
    V3,
}

/// Preserve the old whole-arguments null -> {} convention. An explicitly
/// malformed selector is an error; it must never fall through to version 2.
pub fn select_version(args: Value) -> Result<(ContractVersion, Value), &'static str> {
    let mut args = match args {
        Value::Null => serde_json::Map::new(),
        Value::Object(args) => args,
        _ => return Err("arguments"),
    };
    let version = match args.remove("contractVersion") {
        None => ContractVersion::V2,
        Some(Value::Number(number)) if number.is_u64() => match number.as_u64() {
            Some(2) => ContractVersion::V2,
            Some(3) => ContractVersion::V3,
            _ => return Err("contractVersion"),
        },
        Some(_) => return Err("contractVersion"),
    };
    Ok((version, Value::Object(args)))
}

pub fn legacy_fields(tool: &str) -> Option<&'static [&'static str]> {
    Some(match tool {
        "managed_table_v2_get" | "managed_row_v2_get" => &["id"],
        "managed_tables_v2_list" => &["after"],
        "managed_rows_v2_list" => &["tableId", "after"],
        "managed_table_v2_submit" => &["scope", "crewId", "requestId", "operation"],
        "managed_table_v2_request_get" => &["scope", "crewId", "requestId"],
        _ => return None,
    })
}

/// Generate the actual V3 DTO schema without adding an RPC or copying a union
/// into the tool catalog. All local references are expanded for MCP consumers.
pub fn version_three_schema(tool: &str) -> Option<Value> {
    Some(match tool {
        "managed_table_v2_get" => expanded_schema::<v3::ToolGetV3>(),
        "managed_tables_v2_list" => expanded_schema::<v3::TableList>(),
        "managed_row_v2_get" => expanded_schema::<v3::ToolRowGetV3>(),
        "managed_rows_v2_list" => expanded_schema::<v3::RowList>(),
        "managed_table_v2_submit" => expanded_schema::<v3::Submit>(),
        "managed_table_v2_request_get" => expanded_schema::<v3::RequestGet>(),
        _ => return None,
    })
}

fn expanded_schema<T: JsonSchema>() -> Value {
    let root =
        serde_json::to_value(schemars::schema_for!(T)).expect("serializable protocol schema");
    fn expand(value: &Value, root: &Value, depth: u16) -> Value {
        assert!(depth < 128, "non-recursive table argument schema");
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                    let pointer = reference
                        .strip_prefix('#')
                        .expect("local protocol reference");
                    let target = root.pointer(pointer).expect("resolved protocol reference");
                    let mut result = expand(target, root, depth + 1);
                    for (key, value) in object {
                        if key != "$ref" {
                            result[key] = expand(value, root, depth + 1);
                        }
                    }
                    return result;
                }
                Value::Object(
                    object
                        .iter()
                        .filter(|(key, _)| !matches!(key.as_str(), "$defs" | "$schema"))
                        .map(|(key, value)| (key.clone(), expand(value, root, depth + 1)))
                        .collect(),
                )
            }
            Value::Array(array) => Value::Array(
                array
                    .iter()
                    .map(|value| expand(value, root, depth + 1))
                    .collect(),
            ),
            value => value.clone(),
        }
    }
    expand(&root, &root, 0)
}

/// Merge only disjoint tagged variants with byte-identical remaining schemas.
/// E.g. {type:"work",id:string}|{type:"requirement",id:string} can share the
/// object with type enum ["work","requirement"]. Required/closed bounds remain.
pub fn coalesce_tagged_schema(mut schema: Value) -> Result<Value, &'static str> {
    visit_schema(&mut schema, 0, &mut |node| {
        let Some(variants) = node.get_mut("oneOf").and_then(Value::as_array_mut) else {
            return true;
        };
        let Some(first) = variants
            .first()
            .and_then(|v| v.get("properties"))
            .and_then(Value::as_object)
        else {
            return true;
        };
        let discriminator = first
            .keys()
            .find(|key| {
                let mut tags = std::collections::BTreeSet::new();
                variants.iter().all(|variant| {
                    variant.get("type").and_then(Value::as_str) == Some("object")
                        && variant.get("additionalProperties") == Some(&Value::Bool(false))
                        && variant
                            .get("required")
                            .and_then(Value::as_array)
                            .is_some_and(|required| {
                                required.contains(&Value::String((*key).clone()))
                            })
                        && variant
                            .get("properties")
                            .and_then(|p| p.get(*key))
                            .and_then(|p| p.get("const"))
                            .and_then(Value::as_str)
                            .is_some_and(|tag| tags.insert(tag))
                })
            })
            .cloned();
        let Some(discriminator) = discriminator else {
            return true;
        };
        let mut groups: std::collections::BTreeMap<Vec<u8>, Vec<usize>> = Default::default();
        for (index, variant) in variants.iter().enumerate() {
            let mut body = variant.clone();
            body["properties"][&discriminator]
                .as_object_mut()
                .unwrap()
                .remove("const");
            groups
                .entry(serde_json::to_vec(&body).unwrap())
                .or_default()
                .push(index);
        }
        let mut result = Vec::new();
        for (index, variant) in variants.iter().enumerate() {
            let mut body = variant.clone();
            body["properties"][&discriminator]
                .as_object_mut()
                .unwrap()
                .remove("const");
            let members = &groups[&serde_json::to_vec(&body).unwrap()];
            if members[0] != index {
                continue;
            }
            if members.len() == 1 {
                result.push(variant.clone());
                continue;
            }
            body["properties"][&discriminator]["enum"] = Value::Array(
                members
                    .iter()
                    .map(|&member| variants[member]["properties"][&discriminator]["const"].clone())
                    .collect(),
            );
            result.push(body);
        }
        *variants = result;
        true
    })?;
    Ok(schema)
}

// Visit schema positions only; enum/const data and property names are values.
fn visit_schema(
    value: &mut Value,
    depth: u16,
    visit: &mut dyn FnMut(&mut Value) -> bool,
) -> Result<(), &'static str> {
    if depth >= 128 {
        return Err("table schema depth");
    }
    if !visit(value) {
        return Ok(());
    }
    let Some(object) = value.as_object_mut() else {
        return Ok(());
    };
    for (keyword, child) in object {
        match keyword.as_str() {
            "properties" | "patternProperties" | "dependentSchemas" => {
                if let Some(map) = child.as_object_mut() {
                    for node in map.values_mut() {
                        visit_schema(node, depth + 1, visit)?;
                    }
                }
            }
            "oneOf" | "anyOf" | "allOf" | "prefixItems" => {
                if let Some(array) = child.as_array_mut() {
                    for node in array {
                        visit_schema(node, depth + 1, visit)?;
                    }
                }
            }
            "items" | "additionalProperties" | "contains" | "not" | "if" | "then" | "else" => {
                if child.is_object() {
                    visit_schema(child, depth + 1, visit)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn disjoint_tag_merge_preserves_every_other_constraint_and_literal() {
        let branch = |tag: &str, field: &str| {
            json!({"type":"object","properties":{
            "kind":{"type":"string","const":tag,"description":"same discriminator"},
            field:{"type":"string","maxLength":4096},
        },"required":["kind",field],"additionalProperties":false})
        };
        let a = branch("work", "id");
        let b = branch("requirement", "id");
        let c = branch("message", "messageId");
        let original = json!({"oneOf":[a,b,c],"const":{"oneOf":[a,b]}});
        let merged = coalesce_tagged_schema(original.clone()).unwrap();
        assert_eq!(merged["oneOf"].as_array().unwrap().len(), 2);
        assert_eq!(
            merged["oneOf"][0]["properties"]["kind"]["enum"],
            json!(["work", "requirement"])
        );
        assert_eq!(merged["const"], original["const"]);
        let mut reverse = Vec::new();
        for branch in merged["oneOf"].as_array().unwrap() {
            if let Some(tags) = branch["properties"]["kind"]["enum"].as_array() {
                for tag in tags {
                    let mut restored = branch.clone();
                    restored["properties"]["kind"]
                        .as_object_mut()
                        .unwrap()
                        .remove("enum");
                    restored["properties"]["kind"]["const"] = tag.clone();
                    reverse.push(restored);
                }
            } else {
                reverse.push(branch.clone());
            }
        }
        assert_eq!(Value::Array(reverse), original["oneOf"]);
        let overlap = json!({"oneOf":[a,a,b]});
        assert_eq!(coalesce_tagged_schema(overlap.clone()).unwrap(), overlap);
        let mut optional = original.clone();
        optional["oneOf"][0]["required"] = json!(["id"]);
        assert_eq!(coalesce_tagged_schema(optional.clone()).unwrap(), optional);
    }

    #[test]
    fn omitted_or_integer_two_keeps_original_arguments_and_null_convention() {
        let old = json!({"id":"T2"});
        assert_eq!(
            select_version(old.clone()).unwrap(),
            (ContractVersion::V2, old.clone())
        );
        assert_eq!(
            select_version(json!({"contractVersion":2,"id":"T2"})).unwrap(),
            (ContractVersion::V2, old)
        );
        assert_eq!(
            select_version(Value::Null).unwrap(),
            (ContractVersion::V2, json!({}))
        );
    }

    #[test]
    fn malformed_presence_never_falls_back_to_old_arguments() {
        for value in [
            Value::Null,
            json!("3"),
            json!(3.0),
            json!(true),
            json!(1),
            json!(4),
            json!(-3),
            json!([]),
            json!({}),
        ] {
            assert_eq!(
                select_version(json!({"contractVersion":value,"id":"T2"})),
                Err("contractVersion")
            );
        }
        for value in [json!(false), json!([]), json!("T2")] {
            assert_eq!(select_version(value), Err("arguments"));
        }
    }

    #[test]
    fn explicit_three_requires_the_closed_new_action_without_old_fallback() {
        let (version, args) = select_version(json!({"contractVersion":3,"id":"T2"})).unwrap();
        assert_eq!(version, ContractVersion::V3);
        assert!(serde_json::from_value::<v3::ToolGetV3>(args).is_err());
        let (_, args) = select_version(
            json!({"contractVersion":3,"action":"table","id":"T2","unexpected":false}),
        )
        .unwrap();
        assert!(serde_json::from_value::<v3::ToolGetV3>(args).is_err());
        let (_, args) =
            select_version(json!({"contractVersion":3,"action":"table","id":"T2"})).unwrap();
        assert_eq!(
            serde_json::from_value::<v3::ToolGetV3>(args).unwrap(),
            v3::ToolGetV3::Table { id: "T2".into() }
        );
    }

    #[test]
    fn selectors_do_not_reencode_a_write_or_change_its_identity_and_real_bits() {
        let input: Value = serde_json::from_str(r#"{"contractVersion":2,"requestId":"same","operation":{"kind":"patchRow","value":-0.0},"text":"e\u0301\n\"raw\""}"#).unwrap();
        let mut expected = input.clone();
        expected.as_object_mut().unwrap().remove("contractVersion");
        let (_, selected) = select_version(input).unwrap();
        assert_eq!(selected, expected);
        assert_eq!(
            selected["operation"]["value"].as_f64().unwrap().to_bits(),
            (-0.0f64).to_bits()
        );
        assert_eq!(
            selected["text"].as_str().unwrap().as_bytes(),
            "e\u{301}\n\"raw\"".as_bytes()
        );
    }

    #[test]
    fn each_new_tool_branch_contains_only_resolved_strict_objects() {
        fn strict(value: &Value) {
            if let Some(object) = value.as_object() {
                assert!(!object.contains_key("$ref") && !object.contains_key("$defs"));
                if object.get("type") == Some(&json!("object")) {
                    assert_eq!(
                        object.get("additionalProperties"),
                        Some(&Value::Bool(false))
                    );
                    assert!(object.get("properties").is_some_and(Value::is_object));
                }
                for child in object.values() {
                    strict(child);
                }
            } else if let Some(array) = value.as_array() {
                for child in array {
                    strict(child);
                }
            }
        }
        for tool in [
            "managed_table_v2_get",
            "managed_tables_v2_list",
            "managed_row_v2_get",
            "managed_rows_v2_list",
            "managed_table_v2_submit",
            "managed_table_v2_request_get",
        ] {
            strict(&version_three_schema(tool).unwrap());
            assert!(legacy_fields(tool).is_some());
        }
        assert!(version_three_schema("managed_link_submit").is_none());
        assert!(legacy_fields("managed_link_submit").is_none());
    }
}
