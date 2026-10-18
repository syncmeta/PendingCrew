//! One registry for implemented SQLite knowledge methods.
use crate::{
    agent::{ToolDef, ToolHints},
    team_data::*,
};
use serde_json::{Value, json};
pub const METHODS: &[&str] = &[
    "data.workspace.get",
    "data.submit",
    "data.request.get",
    "data.object.get",
    "data.object.list",
    "data.schema.get",
    "data.query",
    "data.table.get",
    "data.history",
    "data.preview",
    "data.confirm",
    "data.restore",
    "data.snapshots",
    "data.preview.get",
];
pub fn agent_method(name: &str) -> Option<&'static str> {
    METHODS.iter().copied().find(|m| {
        !matches!(
            *m,
            "data.confirm" | "data.restore" | "data.snapshots" | "data.preview.get"
        ) && m.replace('.', "_") == name
    })
}
/// `true` and `{}` are equivalent JSON Schemas. Codex 0.162.1 lowers the
/// boolean form to a string schema, so use the object form for arbitrary JSON.
/// Visit schema positions only; boolean additionalProperties and instance data
/// in enum/const/default must retain their original meaning.
fn normalize_model_schema(schema: &mut Value) {
    if *schema == Value::Bool(true) {
        *schema = json!({});
        return;
    }
    let Some(fields) = schema.as_object_mut() else {
        return;
    };
    for key in ["properties", "$defs", "definitions"] {
        if let Some(children) = fields.get_mut(key).and_then(Value::as_object_mut) {
            for child in children.values_mut() {
                normalize_model_schema(child);
            }
        }
    }
    for key in ["items", "additionalProperties"] {
        if let Some(child) = fields.get_mut(key) {
            if key != "additionalProperties" || !child.is_boolean() {
                normalize_model_schema(child);
            }
        }
    }
    for key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if let Some(children) = fields.get_mut(key).and_then(Value::as_array_mut) {
            for child in children {
                normalize_model_schema(child);
            }
        }
    }
}

// This matches the trusted `crew` configuration in the Codex driver. Keep
// local DAG expansion bounded before advertising schemas to model consumers.
const MODEL_SCHEMA_MAX_BYTES: usize = 32_768;

fn inline_model_schema(root: &Value) -> Result<Value, &'static str> {
    fn expand(
        root: &Value,
        node: &Value,
        refs: &mut Vec<String>,
        depth: usize,
        nodes_left: &mut usize,
    ) -> Result<Value, &'static str> {
        if depth > 64 || *nodes_left == 0 {
            return Err("knowledge schema expansion exceeds structural bounds");
        }
        *nodes_left -= 1;
        let Some(fields) = node.as_object() else {
            return Ok(node.clone());
        };
        // A new schema resource changes the base of fragment references.
        // Our generated schemas have none; never guess that resolution.
        if fields.contains_key("$id") {
            return Err("nested knowledge schema resources are unsupported");
        }
        let result = if let Some(reference) = fields.get("$ref") {
            let reference = reference
                .as_str()
                .ok_or("invalid knowledge schema reference")?;
            let pointer = reference
                .strip_prefix('#')
                .ok_or("non-local knowledge schema reference")?;
            if !pointer.is_empty() && !pointer.starts_with('/') {
                return Err("unsupported knowledge schema fragment");
            }
            if refs.iter().any(|active| active == reference) {
                return Err("cyclic knowledge schema reference");
            }
            let target = root
                .pointer(pointer)
                .ok_or("missing knowledge schema reference")?;
            refs.push(reference.to_string());
            let resolved = expand(root, target, refs, depth + 1, nodes_left);
            refs.pop();
            let resolved = resolved?;
            let siblings = fields
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "$ref" | "$defs" | "definitions"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<serde_json::Map<_, _>>();
            if siblings.is_empty() {
                resolved
            } else {
                // Ref siblings are a conjunction, not a field overwrite.
                json!({"allOf":[resolved, expand(root, &Value::Object(siblings), refs, depth + 1, nodes_left)?]})
            }
        } else {
            let mut output = serde_json::Map::new();
            for (key, value) in fields {
                if matches!(key.as_str(), "$defs" | "definitions") {
                    continue;
                }
                let child = match key.as_str() {
                    "properties" | "patternProperties" | "dependentSchemas" => {
                        let children = value
                            .as_object()
                            .ok_or("invalid knowledge schema property map")?;
                        let mut expanded = serde_json::Map::new();
                        for (name, child) in children {
                            expanded.insert(
                                name.clone(),
                                expand(root, child, refs, depth + 1, nodes_left)?,
                            );
                        }
                        Value::Object(expanded)
                    }
                    "items"
                    | "additionalProperties"
                    | "contains"
                    | "propertyNames"
                    | "not"
                    | "if"
                    | "then"
                    | "else"
                    | "unevaluatedProperties"
                    | "unevaluatedItems" => expand(root, value, refs, depth + 1, nodes_left)?,
                    "anyOf" | "oneOf" | "allOf" | "prefixItems" => {
                        let children = value
                            .as_array()
                            .ok_or("invalid knowledge schema composition")?;
                        let mut expanded = Vec::new();
                        for child in children {
                            expanded.push(expand(root, child, refs, depth + 1, nodes_left)?);
                        }
                        Value::Array(expanded)
                    }
                    // enum, const, defaults and descriptions are instance data.
                    _ => value.clone(),
                };
                output.insert(key.clone(), child);
            }
            Value::Object(output)
        };
        if serde_json::to_vec(&result)
            .map_err(|_| "invalid knowledge schema JSON")?
            .len()
            > MODEL_SCHEMA_MAX_BYTES
        {
            return Err("knowledge model schema exceeds byte budget");
        }
        Ok(result)
    }
    if serde_json::to_vec(root)
        .map_err(|_| "invalid knowledge schema JSON")?
        .len()
        > MODEL_SCHEMA_MAX_BYTES
    {
        return Err("knowledge source schema exceeds byte budget");
    }
    expand(root, root, &mut Vec::new(), 0, &mut 4096)
}

pub fn agent_tools() -> Vec<ToolDef> {
    fn input<T: schemars::JsonSchema>() -> Value {
        // Each MCP tool is a separate schema document, unlike the shared RPC registry.
        // Inline only this argument type's reachable definitions for consumers
        // whose signature renderer limits local-reference expansion.
        let mut schema =
            serde_json::to_value(schemars::schema_for!(T)).expect("static knowledge tool schema");
        normalize_model_schema(&mut schema);
        inline_model_schema(&schema)
            .expect("static knowledge model schemas must be bounded local DAGs")
    }
    vec![
        (METHODS[0], input::<DataWorkspaceGetParams>()),
        (METHODS[1], input::<DataSubmitParams>()),
        (METHODS[2], input::<DataRequestGetParams>()),
        (METHODS[3], input::<DataObjectGetParams>()),
        (METHODS[4], input::<DataObjectListParams>()),
        (METHODS[5], input::<DataSchemaParams>()),
        (METHODS[6], input::<DataQueryParams>()),
        (METHODS[7], input::<DataTableParams>()),
        (METHODS[8], input::<DataHistoryParams>()),
        (METHODS[9], input::<DataPreviewParams>()),
    ].into_iter()
        .map(|(m, s)| ToolDef {
            name: m.replace('.', "_"),
            description: format!("SQLite knowledge {m}. Current crew ACL applies. First call data_workspace_get with crewId to obtain the complete current context; never invent or shorten context fields. An uninitialized workspace has no database to read: an authorized first data_submit apply creates it. Reads, including data_schema_get, do not initialize it. After a committed submit use its returned context for later reads and new writes; data_request_get and exact retries instead require the original request context, including its original generation. SQL/schema reads describe business SQL tables, not blocks or views; use data_object_get/list for knowledge objects. SQL/schema atRevision must be null. Use the original request UUID; never replay an Unknown outcome. Dangerous SQL needs real Human confirmation.{}", if m == "data.submit" { " For executable Mac views use definition kind='web', renderer='web', rendererVersion='1'. Supply your own HTML/CSS/JavaScript and declared bindings. The closed JavaScript bridge is window.knowledge.request(operation,input), resolving {callID,response}; read uses {binding:'bindingId'} and returns response.value. Writes use operation='write' with exactly {message,edits} and require requestedCapabilities:['knowledge-write'] plus separate native Human code-enable and write grants for this version. Readonly views need no write capability. Unknown writes must be looked up through the retained original writeCallID; never retry by inventing a new call ID. The bridge also supports lookup with {writeCallID} and navigate with an authorized Core reference; it grants no arbitrary RPC, network or file access." } else { "" }),
            input_schema: s,
            annotations: Some(ToolHints { read_only_hint: !matches!(m, "data.submit" | "data.preview"), destructive_hint: m == "data.submit", idempotent_hint: true, open_world_hint: false }),
        }).collect()
}
pub(crate) fn schema_entries(g: &mut schemars::SchemaGenerator) -> Vec<(&'static str, Value)> {
    vec![
        (
            METHODS[0],
            json!({"params":crate::sub::<DataWorkspaceGetParams>(g),"result":crate::sub::<DataWorkspaceInfo>(g)}),
        ),
        (
            METHODS[1],
            json!({"params":crate::sub::<DataSubmitParams>(g),"result":crate::sub::<DataChangeResult>(g)}),
        ),
        (
            METHODS[2],
            json!({"params":crate::sub::<DataRequestGetParams>(g),"result":crate::sub::<DataRequestGetResult>(g)}),
        ),
        (
            METHODS[3],
            json!({"params":crate::sub::<DataObjectGetParams>(g),"result":crate::sub::<DataObjectDetail>(g)}),
        ),
        (
            METHODS[4],
            json!({"params":crate::sub::<DataObjectListParams>(g),"result":crate::sub::<DataObjectPage>(g)}),
        ),
        (
            METHODS[5],
            json!({"params":crate::sub::<DataSchemaParams>(g),"result":crate::sub::<DataSchemaResult>(g)}),
        ),
        (
            METHODS[6],
            json!({"params":crate::sub::<DataQueryParams>(g),"result":crate::sub::<DataRows>(g)}),
        ),
        (
            METHODS[7],
            json!({"params":crate::sub::<DataTableParams>(g),"result":crate::sub::<DataTablePage>(g)}),
        ),
        (
            METHODS[8],
            json!({"params":crate::sub::<DataHistoryParams>(g),"result":crate::sub::<DataHistory>(g)}),
        ),
        (
            METHODS[9],
            json!({"params":crate::sub::<DataPreviewParams>(g),"result":crate::sub::<DataPreview>(g)}),
        ),
        (
            METHODS[10],
            json!({"params":crate::sub::<DataConfirmParams>(g),"result":crate::sub::<DataChangeResult>(g)}),
        ),
        (
            METHODS[11],
            json!({"params":crate::sub::<DataRestoreParams>(g),"result":crate::sub::<DataChangeResult>(g)}),
        ),
        (
            METHODS[12],
            json!({"params":crate::sub::<DataSnapshotsParams>(g),"result":crate::sub::<DataSnapshots>(g)}),
        ),
        (
            METHODS[13],
            json!({"params":crate::sub::<DataPreviewGetParams>(g),"result":crate::sub::<DataPreview>(g)}),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertised_knowledge_tools_have_self_contained_object_schemas() {
        fn check_refs(root: &Value, value: &Value, tool: &str) {
            match value {
                Value::Object(fields) => {
                    if let Some(reference) = fields.get("$ref").and_then(Value::as_str) {
                        let pointer = reference
                            .strip_prefix('#')
                            .expect("local tool schema reference");
                        assert!(
                            root.pointer(pointer).is_some(),
                            "{tool}: unresolved {reference}"
                        );
                    }
                    for child in fields.values() {
                        check_refs(root, child, tool);
                    }
                }
                Value::Array(items) => {
                    for child in items {
                        check_refs(root, child, tool);
                    }
                }
                _ => {}
            }
        }
        let tools = agent_tools();
        assert_eq!(tools.len(), 10);
        for tool in tools {
            assert_eq!(
                tool.input_schema["type"], "object",
                "{} has no object input schema",
                tool.name
            );
            check_refs(&tool.input_schema, &tool.input_schema, &tool.name);
            let required = tool.input_schema["required"]
                .as_array()
                .expect("typed required fields");
            let identity = if tool.name == "data_workspace_get" {
                "crewId"
            } else {
                "context"
            };
            assert!(
                required.iter().any(|field| field == identity),
                "{} must describe {identity}",
                tool.name
            );
            if tool.name == "data_submit" || tool.name == "data_request_get" {
                assert!(required.iter().any(|field| field == "requestId"));
            }
            assert!(
                tool.input_schema["properties"].is_object(),
                "{} has no described input fields",
                tool.name
            );
            assert_eq!(
                tool.input_schema["additionalProperties"], false,
                "{} must retain strict typed arguments",
                tool.name
            );
        }
    }
    #[test]
    fn arbitrary_json_tool_inputs_remain_unrestricted_schemas() {
        let submit = agent_tools()
            .into_iter()
            .find(|t| t.name == "data_submit")
            .unwrap()
            .input_schema;
        let change = &submit["properties"]["change"]["oneOf"];
        let edits = &change[0]["properties"]["edits"]["items"]["oneOf"];
        let create = &edits[0]["properties"];
        assert_eq!(create["content"], json!({}));
        assert_eq!(create["attributes"], json!({}));
        assert_eq!(edits.as_array().unwrap().len(), 10);
        assert_eq!(change.as_array().unwrap().len(), 2);
    }
    #[test]
    fn model_schema_normalization_does_not_rewrite_instance_data() {
        let instance = json!({"properties":{"literal":true},"items":true});
        let mut schema = json!({
            "type":"object", "additionalProperties":true,
            "const":instance, "enum":[instance], "default":instance,
            "properties":{"payload":true,"denied":false,"array":{"items":true},
                "map":{"additionalProperties":{"properties":{"payload":true}}}},
            "$defs":{"choice":{"oneOf":[true,{"properties":{"payload":true}}]}}
        });
        normalize_model_schema(&mut schema);
        assert_eq!(schema["properties"]["payload"], json!({}));
        assert_eq!(schema["properties"]["denied"], false);
        assert_eq!(schema["properties"]["array"]["items"], json!({}));
        assert_eq!(
            schema["properties"]["map"]["additionalProperties"]["properties"]["payload"],
            json!({})
        );
        assert_eq!(schema["$defs"]["choice"]["oneOf"][0], json!({}));
        assert_eq!(
            schema["$defs"]["choice"]["oneOf"][1]["properties"]["payload"],
            json!({})
        );
        assert_eq!(schema["additionalProperties"], true);
        assert_eq!(schema["const"], instance);
        assert_eq!(schema["enum"], json!([instance]));
        assert_eq!(schema["default"], instance);
        for tool in agent_tools() {
            assert!(
                serde_json::to_vec(&tool.input_schema).unwrap().len() <= MODEL_SCHEMA_MAX_BYTES,
                "{} exceeds our supported Codex schema budget",
                tool.name
            );
        }
    }
    #[test]
    fn advertised_model_inputs_need_no_local_reference_expansion() {
        fn check(value: &Value) {
            match value {
                Value::Object(fields) => {
                    assert!(!fields.contains_key("$ref"));
                    assert!(!fields.contains_key("$defs"));
                    for child in fields.values() {
                        check(child);
                    }
                }
                Value::Array(values) => {
                    for child in values {
                        check(child);
                    }
                }
                _ => {}
            }
        }
        for tool in agent_tools() {
            check(&tool.input_schema);
        }
    }

    #[test]
    fn local_schema_expansion_is_lossless_and_fails_closed() {
        let original = json!({"$defs":{"scalar":{"type":"string"}},"type":"object",
            "additionalProperties":false,"required":["value"],
            "properties":{"value":{"$ref":"#/$defs/scalar","enum":["yes"]}},
            "const":{"$ref":"literal instance","properties":{"literal":true}}});
        let expanded = inline_model_schema(&original).unwrap();
        assert_eq!(
            expanded["properties"]["value"],
            json!({"allOf":[{"type":"string"},{"enum":["yes"]}]})
        );
        assert_eq!(expanded["additionalProperties"], false);
        assert_eq!(expanded["required"], json!(["value"]));
        assert_eq!(expanded["const"], original["const"]);
        assert!(inline_model_schema(&json!({"$ref":"#/$defs/absent"})).is_err());
        assert!(
            inline_model_schema(
                &json!({"$defs":{"loop":{"$ref":"#/$defs/loop"}},"$ref":"#/$defs/loop"})
            )
            .is_err()
        );
        assert!(inline_model_schema(&json!({"$ref":"https://example.invalid/schema"})).is_err());
        assert!(
            inline_model_schema(
                &json!({"type":"string","description":"x".repeat(MODEL_SCHEMA_MAX_BYTES)})
            )
            .is_err()
        );
        let properties = (0..20)
            .map(|n| (format!("field{n}"), json!({"$ref":"#/$defs/wide"})))
            .collect::<serde_json::Map<_, _>>();
        let wide = json!({"$defs":{"wide":{"type":"string","description":"x".repeat(2000)}},
            "type":"object","properties":properties});
        assert!(serde_json::to_vec(&wide).unwrap().len() < MODEL_SCHEMA_MAX_BYTES);
        assert!(inline_model_schema(&wide).is_err());
        let mut deep = json!({"type":"string"});
        for _ in 0..66 {
            deep = json!({"type":"array","items":deep});
        }
        assert!(inline_model_schema(&deep).is_err());
    }
}
