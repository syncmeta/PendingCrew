use crate::Ctx;
use crate::model::LAttachment;
use chrono::{DateTime, SecondsFormat, Utc};
use crewd_proto::Attachment;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::Path;

/// 读一个「顶层是数组」的账本文件。不存在 → None（不报）；读不出/不是 JSON/不是数组 →
/// None 并记一条整文件跳过（不标 consumed，归档会原样保存它）。
pub fn read_array(cx: &mut Ctx, path: &Path) -> Option<Vec<Value>> {
    if !path.exists() {
        return None;
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            cx.report.skip(cx.src, path, None, format!("读取失败: {e}"));
            return None;
        }
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Array(rows)) => {
            cx.consumed.insert(path.to_path_buf());
            Some(rows)
        }
        Ok(_) => {
            cx.report.skip(cx.src, path, None, "顶层不是数组");
            None
        }
        Err(e) => {
            cx.report
                .skip(cx.src, path, None, format!("JSON 无效: {e}"));
            None
        }
    }
}

/// 逐行解码；坏行记跳过（带下标），其余照收。
pub fn read_rows<T: DeserializeOwned>(cx: &mut Ctx, path: &Path) -> Vec<T> {
    read_rows_indexed(cx, path)
        .into_iter()
        .map(|(_, t)| t)
        .collect()
}

/// 同 `read_rows`，但每行带它在源数组里的下标，便于后续阶段按行报告。
pub fn read_rows_indexed<T: DeserializeOwned>(cx: &mut Ctx, path: &Path) -> Vec<(usize, T)> {
    let Some(values) = read_array(cx, path) else {
        return vec![];
    };
    let mut out = Vec::with_capacity(values.len());
    let mut any_bad = false;
    for (i, v) in values.into_iter().enumerate() {
        match serde_json::from_value::<T>(v) {
            Ok(t) => out.push((i, t)),
            Err(e) => {
                any_bad = true;
                cx.report.skip(cx.src, path, Some(i), e.to_string());
            }
        }
    }
    if any_bad {
        // 有行没进库：整份文件留给归档原样保存（解出来的行照常导入）。
        cx.consumed.remove(path);
    }
    out
}

/// 统一成 UTC 毫秒 RFC3339。解析不了就原样保留并计数——宁可排序略乱，也不丢行。
pub fn norm_ts(cx: &mut Ctx, raw: &str) -> String {
    match DateTime::parse_from_rfc3339(raw.trim()) {
        Ok(t) => t
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
        Err(_) => {
            cx.report.bump("timestampsUnparsed");
            raw.to_string()
        }
    }
}

pub fn conv_att(cx: &mut Ctx, a: LAttachment) -> Attachment {
    let path = cx.rewrite.apply(&a.path);
    if path == a.path {
        cx.report.bump("attachmentPathsNotRewritten");
    }
    Attachment {
        id: a.id,
        mime: a.mime,
        size: a.size,
        path,
        filename: a.filename,
    }
}
