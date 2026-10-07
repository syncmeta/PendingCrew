//! 群 id 规则与导入 / 迁移要用的最少行函数。群的规则在 crewd-collab。
use crate::Result;
use rusqlite::{Connection, params};

/// 机组大厅的 id（确定性，spec §22.2）。
pub fn lobby_id(crew_id: &str) -> String {
    format!("{crew_id}/lobby")
}

/// home（总机组 / 一级机组）人类群的 id（确定性；懒建）。
pub fn human_id(home_crew_id: &str) -> String {
    format!("{home_crew_id}/human")
}

/// 确保机组有大厅（已有就不动）。返回大厅 id。
pub fn ensure_lobby(conn: &Connection, crew_id: &str, created_at: &str) -> Result<String> {
    let id = lobby_id(crew_id);
    conn.execute(
        "INSERT OR IGNORE INTO channels(id,crew_id,kind,name,created_by,created_at)
         VALUES(?1,?2,'lobby','','system',?3)",
        params![id, crew_id, created_at],
    )?;
    Ok(id)
}
