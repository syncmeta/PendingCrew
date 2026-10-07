//! 全机通讯录（移植 `CrewDirectory.swift:125-170`）：机组按号升序，每个机组后面跟机长（分机 1）与有号的成员。
use crate::refusal::{Code, Refusal};
use crate::{CollabError, Result, crews, sessions};
use crewd_proto::{DirectoryKind, DirectoryRow};
use rusqlite::{Connection, OptionalExtension, params};

/// 通讯录地址 → session id（M3b `post.to` 用）：`"N-E"` = N 号机组分机 E 的 session；`"N"` / `"N-1"` = N 号机组
/// **此刻**的机长（分机 1 = 机长，跟着机长指针走）。号码只认十进制数字。
/// 格式不对 → `Invalid`；没有这个号的机组 / 分机 → `NotFound`；机组此刻没有机长 → `Invalid`；
/// 机组已归档（N1 裁定 13）→ `Conflict`「已归档」（号还在、不回收，但不再收消息；不说成「没有这个号」）。
/// 号码没有唯一索引：撞号时取最早建的机组（再按 id），与通讯录里的先后一致。
pub fn resolve(conn: &Connection, address: &str) -> Result<String> {
    let bad =
        || CollabError::Invalid(Refusal::new(Code::DirectoryBadAddress).arg("address", address));
    let num = |s: &str| -> Result<i64> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        s.parse().map_err(|_| bad())
    };
    let (n, e) = match address.split_once('-') {
        Some((n, e)) => (num(n)?, Some(num(e)?)),
        None => (num(address)?, None),
    };
    let crew: String = conn
        .query_row(
            "SELECT id FROM crews WHERE number=?1 ORDER BY created_at, id LIMIT 1",
            [n],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| {
            CollabError::NotFound(
                Refusal::new(Code::DirectoryNoSuchCrew).arg("number", n.to_string()),
            )
        })?;
    if crews::is_archived(conn, &crew)? {
        return Err(CollabError::Conflict(
            Refusal::new(Code::DirectoryCrewRemoved).arg("number", n.to_string()),
        ));
    }
    match e {
        None | Some(1) => crews::captain_of(conn, &crew)?.ok_or_else(|| {
            CollabError::Invalid(Refusal::new(Code::CrewNoCaptain).arg("crew", crew.clone()))
        }),
        Some(e) => conn
            .query_row(
                "SELECT id FROM sessions WHERE crew_id=?1 AND ext=?2 ORDER BY id LIMIT 1",
                params![crew, e],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| {
                CollabError::NotFound(
                    Refusal::new(Code::DirectoryNoSuchExtension)
                        .arg("number", n.to_string())
                        .arg("ext", e.to_string()),
                )
            }),
    }
}

/// 组织路径：从顶层机组到 `crew` 的**上级**，" / " 分隔。每层只取最早建的那个父（`crews::parents` 的第一个）。
/// 顶层机组与总机组（不在树里）的路径是空的；总机组仍有 "0" 那一行。
/// 多父机组里这条链与 `home_of`（在全部顶层祖先里取最早建的）不保证经过同一个顶层机组——两者是各自独立的口径。
fn org_path(conn: &Connection, crew: &str) -> Result<String> {
    let mut titles = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut cur = crew.to_string();
    while let Some(p) = crews::parents(conn, &cur)?.into_iter().next() {
        if !seen.insert(p.clone()) {
            break;
        }
        titles.push(crews::get(conn, &p)?.title);
        cur = p;
    }
    titles.reverse();
    Ok(titles.join(" / "))
}

/// 归档的机组不列（与 `crew.list` 默认同口径；它的号也不再解析，见 `resolve`）。
pub fn list(conn: &Connection, query: Option<&str>) -> Result<Vec<DirectoryRow>> {
    let mut all = Vec::new();
    let mut numbered: Vec<_> = crews::list(conn, false)?
        .into_iter()
        .filter(|c| c.number.is_some())
        .collect();
    numbered.sort_by_key(|c| c.number);
    for c in numbered {
        let n = c.number.expect("filtered");
        let path = org_path(conn, &c.id)?;
        let self_path = if path.is_empty() {
            c.title.clone()
        } else {
            format!("{path} / {}", c.title)
        };
        all.push(DirectoryRow {
            number: n.to_string(),
            kind: DirectoryKind::Crew,
            name: c.title.clone(),
            crew_id: c.id.clone(),
            session_id: None,
            org_path: path,
        });
        let captain = crews::captain_of(conn, &c.id)?;
        if let Some(cap) = &captain {
            all.push(DirectoryRow {
                number: format!("{n}-1"),
                kind: DirectoryKind::Session,
                name: sessions::display_name(conn, cap)?,
                crew_id: c.id.clone(),
                session_id: Some(cap.clone()),
                org_path: self_path.clone(),
            });
        }
        let mut members: Vec<_> = sessions::list(conn, Some(&c.id))?
            .into_iter()
            .filter(|s| s.ext.is_some() && Some(&s.id) != captain.as_ref())
            .collect();
        members.sort_by_key(|s| s.ext);
        for s in members {
            all.push(DirectoryRow {
                number: format!("{n}-{}", s.ext.expect("filtered")),
                kind: DirectoryKind::Session,
                name: sessions::name_of(&s),
                crew_id: c.id.clone(),
                session_id: Some(s.id),
                org_path: self_path.clone(),
            });
        }
    }
    let q = query.unwrap_or("").trim().to_lowercase();
    if q.is_empty() {
        return Ok(all);
    }
    let hit = |r: &DirectoryRow| {
        r.number.starts_with(&q)
            || r.name.to_lowercase().contains(&q)
            || r.org_path.to_lowercase().contains(&q)
    };
    let hit_crews: std::collections::BTreeSet<String> = all
        .iter()
        .filter(|r| hit(r))
        .map(|r| r.crew_id.clone())
        .collect();
    Ok(all
        .into_iter()
        .filter(|r| hit(r) || (r.kind == DirectoryKind::Crew && hit_crews.contains(&r.crew_id)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::store;

    fn numbers(rows: &[DirectoryRow]) -> Vec<&str> {
        rows.iter().map(|r| r.number.as_str()).collect()
    }

    #[test]
    fn full_table_lists_crews_captains_and_numbered_members() {
        let s = store();
        let rows = list(s.conn(), None).unwrap();
        assert_eq!(
            numbers(&rows),
            ["0", "1", "1-1", "1-2", "1-3", "2", "2-1", "2-2", "3", "3-1"]
        );
        let root = rows.iter().find(|r| r.number == "0").unwrap();
        assert_eq!(
            (root.crew_id.as_str(), root.org_path.as_str()),
            ("chief", "")
        );
        let a = rows.iter().find(|r| r.number == "1").unwrap();
        assert_eq!(a.org_path, "", "总机组不进组织路径");
        let b = rows.iter().find(|r| r.number == "2").unwrap();
        assert_eq!(b.org_path, "机组甲");
        let b1 = rows.iter().find(|r| r.number == "2-2").unwrap();
        assert_eq!(b1.org_path, "机组甲 / 机组乙");
        assert_eq!(b1.session_id.as_deref(), Some("w-b1"));
        assert_eq!(
            rows.iter().find(|r| r.number == "1-1").unwrap().name,
            "名字-cap-a"
        );
    }

    #[test]
    fn query_matches_number_prefix_or_name_and_keeps_the_crew_header() {
        let s = store();
        assert_eq!(
            numbers(&list(s.conn(), Some("名字-w-b1")).unwrap()),
            ["2", "2-2"]
        );
        assert_eq!(numbers(&list(s.conn(), Some("3")).unwrap()), ["3", "3-1"]);
        assert!(list(s.conn(), Some("不存在的名字")).unwrap().is_empty());
        assert_eq!(list(s.conn(), Some("  ")).unwrap().len(), 10, "空白 = 全表");
        assert!(
            list(s.conn(), Some("总机组"))
                .unwrap()
                .iter()
                .all(|r| r.crew_id == "chief"),
            "组织路径里没有总机组：按它的名字只搜到它自己"
        );
    }

    #[test]
    fn an_address_resolves_to_a_session_or_a_crews_current_captain() {
        let s = store();
        assert_eq!(resolve(s.conn(), "1-2").unwrap(), "w-a1");
        assert_eq!(resolve(s.conn(), "1").unwrap(), "cap-a");
        assert_eq!(resolve(s.conn(), "1-1").unwrap(), "cap-a");
        assert!(matches!(
            resolve(s.conn(), "9").unwrap_err(),
            CollabError::NotFound(_)
        ));
        assert!(matches!(
            resolve(s.conn(), "1-9").unwrap_err(),
            CollabError::NotFound(_)
        ));
        assert!(
            matches!(resolve(s.conn(), "0").unwrap_err(), CollabError::Invalid(_)),
            "总机组现在没有机长"
        );
        assert!(matches!(
            resolve(s.conn(), "甲").unwrap_err(),
            CollabError::Invalid(_)
        ));
    }

    /// 归档的机组不在通讯录里，它的号（含分机）也不再解析：报「已归档」，不是「没有这个号」。
    #[test]
    fn an_archived_crew_is_out_of_the_directory_and_its_number_does_not_resolve() {
        let mut s = store();
        crate::testkit::ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
        assert_eq!(
            numbers(&list(s.conn(), None).unwrap()),
            ["0", "1", "1-1", "1-2", "1-3", "2", "2-1", "2-2"]
        );
        for addr in ["3", "3-1"] {
            match resolve(s.conn(), addr).unwrap_err() {
                CollabError::Conflict(r) => {
                    assert_eq!(r.code, crate::refusal::Code::DirectoryCrewRemoved, "{addr}")
                }
                e => panic!("{addr}: {e:?}"),
            }
        }
        crate::testkit::ok(&mut s, |tx| crate::crews::unarchive(tx, "crew-x"));
        assert_eq!(resolve(s.conn(), "3").unwrap(), "cap-x");
    }
}
