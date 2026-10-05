//! 旧数据里本里程碑不建模的文件（计划板、sweep、续跑凭证、侧边栏排序……）原样存档，
//! 后续里程碑（M4 迁移计划板等）从这里读。
use crate::Result;
use rusqlite::{Connection, OptionalExtension, params};

pub fn put(conn: &Connection, path: &str, content: &[u8]) -> Result<()> {
    conn.execute(
        "INSERT INTO legacy_archive(path,content) VALUES(?1,?2)
         ON CONFLICT(path) DO UPDATE SET content=excluded.content",
        params![path, content],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, path: &str) -> Result<Option<Vec<u8>>> {
    Ok(conn
        .query_row(
            "SELECT content FROM legacy_archive WHERE path=?1",
            [path],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn list_paths(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM legacy_archive ORDER BY path")?;
    let v = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(v)
}
