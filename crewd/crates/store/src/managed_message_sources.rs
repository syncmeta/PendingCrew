//! Core-created source snapshots. Source text never enters object receipts.
use crate::{Result, managed_objects};
use rusqlite::{Connection, OptionalExtension, params};

pub const DDL: &str = r#"
CREATE TABLE managed_message_sources (
    number INTEGER PRIMARY KEY REFERENCES object_refs(number) ON DELETE CASCADE,
    source_type TEXT NOT NULL,
    original_message_id TEXT NOT NULL,
    message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
    channel_id TEXT REFERENCES channels(id) ON DELETE SET NULL,
    excerpt TEXT NOT NULL CHECK(length(excerpt)<=280),
    UNIQUE(source_type,original_message_id)
);
CREATE INDEX managed_sources_by_message ON managed_message_sources(message_id,number);
CREATE TABLE managed_message_receipts (
    crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
    principal TEXT NOT NULL,
    request_id TEXT NOT NULL,
    object_number INTEGER NOT NULL REFERENCES object_refs(number) ON DELETE CASCADE,
    digest BLOB NOT NULL CHECK(length(digest)=32),
    result_json TEXT NOT NULL CHECK(json_valid(result_json)),
    PRIMARY KEY(crew_id,principal,request_id)
);
"#;

pub struct Snapshot {
    pub message_id: Option<String>,
    pub channel_id: Option<String>,
    pub excerpt: String,
}

pub fn get(c: &Connection, number: i64) -> Result<Option<Snapshot>> {
    Ok(c.query_row(
        "SELECT message_id,channel_id,excerpt FROM managed_message_sources WHERE number=?1",
        [number],
        |row| {
            Ok(Snapshot {
                message_id: row.get(0)?,
                channel_id: row.get(1)?,
                excerpt: row.get(2)?,
            })
        },
    )
    .optional()?)
}

pub fn existing(c: &Connection, source_type: &str, message: &str) -> Result<Option<String>> {
    Ok(c.query_row(
        "SELECT r.kind,r.number FROM managed_message_sources s JOIN object_refs r ON r.number=s.number
         WHERE s.source_type=?1 AND s.original_message_id=?2",
        params![source_type,message],
        |row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?)),
    ).optional()?.map(|(kind,number)| {
        let prefix = match kind.as_str() {
            "decision" => "D", "risk" => "R", "lesson" => "L", "custom_row" => "O",
            _ => return Err(crate::StoreError::Invalid("invalid source object kind".into())),
        };
        Ok(format!("{prefix}{number}"))
    }).transpose()?)
}

pub fn record(
    c: &Connection,
    number: i64,
    source_type: &str,
    message: &crewd_proto::Message,
) -> Result<()> {
    managed_objects::transaction(c)?;
    let excerpt: String = message.text.chars().take(280).collect();
    c.execute(
        "INSERT INTO managed_message_sources(number,source_type,original_message_id,message_id,channel_id,excerpt)
         VALUES(?1,?2,?3,?3,?4,?5)",
        params![number,source_type,message.id,message.channel_id,excerpt],
    )?;
    Ok(())
}

pub fn numbers(c: &Connection, message: &str, after: i64, limit: u16) -> Result<Vec<i64>> {
    Ok(c.prepare(
        "SELECT number FROM managed_message_sources WHERE message_id=?1 AND number>?2 ORDER BY number LIMIT ?3",
    )?.query_map(params![message,after,limit],|row| row.get(0))?.collect::<rusqlite::Result<_>>()?)
}

pub fn receipt(
    c: &Connection,
    crew: &str,
    principal: &str,
    request: &str,
) -> Result<
    Option<(
        [u8; 32],
        crewd_proto::managed_message_sources::RecordReceipt,
    )>,
> {
    let saved = c.query_row(
        "SELECT r.digest,r.result_json,r.object_number,s.original_message_id,s.source_type
         FROM managed_message_receipts r LEFT JOIN managed_message_sources s ON s.number=r.object_number
         WHERE r.crew_id=?1 AND r.principal=?2 AND r.request_id=?3",
        params![crew,principal,request],
        |row| Ok((row.get::<_,Vec<u8>>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?,row.get::<_,Option<String>>(3)?,row.get::<_,Option<String>>(4)?)),
    ).optional()?;
    saved
        .map(|(digest, json, number, source_message, source_type)| {
            let digest = digest
                .try_into()
                .map_err(|_| crate::StoreError::Invalid("invalid source receipt digest".into()))?;
            let receipt: crewd_proto::managed_message_sources::RecordReceipt =
                serde_json::from_str(&json)?;
            use crewd_proto::managed_objects::Object;
            let (id, prefix, expected_type) = match &receipt.object {
                Object::Decision { decision } => (&decision.id, "D", "decision".into()),
                Object::Risk { risk } => (&risk.id, "R", "risk".into()),
                Object::Lesson { lesson } => (&lesson.id, "L", "lesson".into()),
                Object::Row { row } => (&row.id, "O", format!("row:{}", row.table_id)),
                Object::Table { .. } => {
                    return Err(crate::StoreError::Invalid(
                        "source receipt object kind mismatch".into(),
                    ));
                }
            };
            if number <= 0
                || *id != format!("{prefix}{number}")
                || source_message.as_deref() != Some(receipt.source_message_id.as_str())
                || source_type.as_deref() != Some(expected_type.as_str())
            {
                return Err(crate::StoreError::Invalid(
                    "source receipt has no matching snapshot".into(),
                ));
            }
            if receipt.crew_id != crew || receipt.request_id != request {
                return Err(crate::StoreError::Invalid(
                    "source receipt scope mismatch".into(),
                ));
            }
            Ok((digest, receipt))
        })
        .transpose()
}

pub fn record_receipt(
    c: &Connection,
    principal: &str,
    number: i64,
    digest: &[u8; 32],
    receipt: &crewd_proto::managed_message_sources::RecordReceipt,
) -> Result<()> {
    managed_objects::transaction(c)?;
    c.execute(
        "INSERT INTO managed_message_receipts(crew_id,principal,request_id,object_number,digest,result_json) VALUES(?1,?2,?3,?4,?5,?6)",
        params![receipt.crew_id,principal,receipt.request_id,number,digest.as_slice(),serde_json::to_string(receipt)?],
    )?;
    Ok(())
}
