//! Lossless supported SDK returned-row to ACP update mapping.
//! This decodes the actual SDK declaration surface, not an unverified JSONL file.
//! It never constructs source completeness or grants session/load capability.
use serde_json::{Value, json};

pub const MAX_TEXT_CHUNK_BYTES: usize = 64 * 1024;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryError {
    Malformed,
    ForeignRow,
    Unsupported(&'static str),
    Oversize,
}
fn object<'a>(
    v: &'a Value,
    allowed: &[&str],
) -> Result<&'a serde_json::Map<String, Value>, HistoryError> {
    let o = v.as_object().ok_or(HistoryError::Malformed)?;
    if o.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(HistoryError::Unsupported("unknown native field"));
    }
    Ok(o)
}
fn text_chunks(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return vec![text];
    }
    let mut out = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let mut end = (offset + MAX_TEXT_CHUNK_BYTES).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        out.push(&text[offset..end]);
        offset = end;
    }
    out
}
/// The entire row is validated before any update escapes. Partial mapping of a
/// row with a later unknown/media block would silently truncate its history.
pub fn returned_row(expected_uuid: &str, bytes: &[u8]) -> Result<Vec<Value>, HistoryError> {
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(HistoryError::Oversize);
    }
    let row: Value = serde_json::from_slice(bytes).map_err(|_| HistoryError::Malformed)?;
    let row = object(&row, &["uuid", "message"])?;
    if row.get("uuid").and_then(Value::as_str) != Some(expected_uuid) {
        return Err(HistoryError::ForeignRow);
    }
    let message = row.get("message").ok_or(HistoryError::Malformed)?;
    let m = object(message, &["type", "name", "role", "isMeta", "content"])?;
    let kind = m
        .get("type")
        .and_then(Value::as_str)
        .ok_or(HistoryError::Malformed)?;
    let role = m.get("role").and_then(Value::as_str);
    let update = match (kind, role) {
        ("user", Some("user")) => "user_message_chunk",
        ("assistant", Some("assistant")) => "agent_message_chunk",
        _ => return Err(HistoryError::Unsupported("non-conversation native row")),
    };
    if m.contains_key("name") {
        return Err(HistoryError::Unsupported("native attachment rendering"));
    }
    if m.get("isMeta").is_some_and(|v| v != &json!(true)) {
        return Err(HistoryError::Malformed);
    }
    let blocks = m
        .get("content")
        .and_then(Value::as_array)
        .ok_or(HistoryError::Malformed)?;
    let mut updates = Vec::new();
    for block in blocks {
        let block = block.as_object().ok_or(HistoryError::Malformed)?;
        let block_kind = block
            .get("type")
            .and_then(Value::as_str)
            .ok_or(HistoryError::Malformed)?;
        match block_kind {
            "text" => {
                if block.keys().any(|k| !matches!(k.as_str(), "type" | "text")) {
                    return Err(HistoryError::Unsupported("text metadata"));
                }
                let text = block
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or(HistoryError::Malformed)?;
                for chunk in text_chunks(text) {
                    updates.push(
                        json!({"sessionUpdate":update,"content":{"type":"text","text":chunk}}),
                    );
                }
            }
            "tool_use" if kind == "assistant" => {
                if block
                    .keys()
                    .any(|k| !matches!(k.as_str(), "type" | "id" | "name" | "input"))
                {
                    return Err(HistoryError::Unsupported("tool call metadata"));
                }
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or(HistoryError::Malformed)?;
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or(HistoryError::Malformed)?;
                let input = block.get("input").ok_or(HistoryError::Malformed)?;
                updates.push(json!({"sessionUpdate":"tool_call","toolCallId":id,"title":name,"kind":"other","status":"pending","rawInput":input}));
            }
            "tool_result" if kind == "user" => {
                if block
                    .keys()
                    .any(|k| !matches!(k.as_str(), "type" | "tool_use_id" | "content" | "is_error"))
                {
                    return Err(HistoryError::Unsupported("tool result metadata"));
                }
                let id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or(HistoryError::Malformed)?;
                let failed = match block.get("is_error") {
                    None => false,
                    Some(v) => v.as_bool().ok_or(HistoryError::Malformed)?,
                };
                let output = block.get("content").ok_or(HistoryError::Malformed)?;
                let mut texts = Vec::new();
                if let Some(s) = output.as_str() {
                    texts.extend(text_chunks(s));
                } else {
                    for item in output.as_array().ok_or(HistoryError::Malformed)? {
                        let item = object(item, &["type", "text"])?;
                        if item.get("type").and_then(Value::as_str) != Some("text") {
                            return Err(HistoryError::Unsupported("tool result media"));
                        }
                        texts.extend(text_chunks(
                            item.get("text")
                                .and_then(Value::as_str)
                                .ok_or(HistoryError::Malformed)?,
                        ));
                    }
                }
                // Output content is losslessly split; final status follows every
                // chunk, so no early completed update hides later output.
                for text in texts {
                    updates.push(json!({"sessionUpdate":"tool_call_update","toolCallId":id,"content":[{"type":"content","content":{"type":"text","text":text}}]}));
                }
                updates.push(json!({"sessionUpdate":"tool_call_update","toolCallId":id,"status":if failed {"failed"} else {"completed"}}));
            }
            _ => {
                return Err(HistoryError::Unsupported(
                    "native block has no lossless ACP mapping",
                ));
            }
        }
    }
    if m.get("isMeta") == Some(&json!(true)) {
        for value in &mut updates {
            value["_meta"] = json!({"pendingcrew.native":{"isMeta":true,"type":kind,"role":role}});
        }
    }
    // The control frame limit applies after JSON encoding, including any tool
    // input. Unknown/oversized structured inputs do not escape as partial replay.
    for value in &updates {
        if serde_json::to_vec(value)
            .map_err(|_| HistoryError::Malformed)?
            .len()
            > super::wire::MAX_CONTROL - 4096
        {
            return Err(HistoryError::Oversize);
        }
    }
    Ok(updates)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(kind: &str, content: Value) -> Vec<u8> {
        serde_json::to_vec(
            &json!({"uuid":"native-row","message":{"type":kind,"role":kind,"content":content}}),
        )
        .unwrap()
    }
    #[test]
    fn legal_mib_body_is_utf8_lossless_and_every_encoded_frame_is_bounded() {
        let text = "🛫\u{0}".repeat(209715);
        let bytes = row("user", json!([{"type":"text","text":text}]));
        let updates = returned_row("native-row", &bytes).unwrap();
        let reconstructed = updates
            .iter()
            .map(|u| u["content"]["text"].as_str().unwrap())
            .collect::<String>();
        assert_eq!(reconstructed, text);
        assert!(updates.len() > 1);
        for update in updates {
            assert!(serde_json::to_vec(&update).unwrap().len() < super::super::wire::MAX_CONTROL);
        }
    }
    #[test]
    fn later_unknown_media_refuses_the_entire_row_without_text_prefix() {
        assert_eq!(
            returned_row(
                "native-row",
                &row(
                    "assistant",
                    json!([{"type":"text","text":"known prefix"},{"type":"image","source":{}}])
                )
            ),
            Err(HistoryError::Unsupported(
                "native block has no lossless ACP mapping"
            ))
        );
    }
    #[test]
    fn tools_preserve_order_ids_input_output_and_actual_error_status() {
        let call=returned_row("native-row",&row("assistant",json!([{"type":"tool_use","id":"tool-id","name":"Bash","input":{"command":"false"}}]))).unwrap();
        assert_eq!(call[0]["rawInput"], json!({"command":"false"}));
        let result=returned_row("native-row",&row("user",json!([{"type":"tool_result","tool_use_id":"tool-id","content":"真实 stderr","is_error":true}]))).unwrap();
        assert_eq!(result[0]["content"][0]["content"]["text"], "真实 stderr");
        assert_eq!(result[1]["status"], "failed");
        assert_eq!(result[1]["toolCallId"], "tool-id");
    }
}
