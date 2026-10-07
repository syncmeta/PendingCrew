//! session 行：注册（分机号）与 M3c / M3b 要写的生命周期字段（裁定 16）。
use crate::{Change, CollabError, Result, Tx, crews, cursors};
use crewd_proto::{Desired, Role, Session};
use rusqlite::{Connection, params};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewSession {
    pub crew_id: String,
    pub role: Role,
    /// "claude" | "codex" | "acp:<registry-id>"（spec §16）。
    pub executor: String,
    pub title: Option<String>,
    pub brief: Option<String>,
    pub display_name: Option<String>,
    /// 缺省 = 机组的工作目录。
    pub working_directory: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// C8：M3c 建好 worktree 后把绝对路径放这里再 `register`；`None` = 共享机组目录。
    pub worktree: Option<String>,
}

/// session 标题最长字符数（S80）。
pub const TITLE_MAX_CHARS: usize = 18;

/// 没给标题时从 brief 派生：第一条去空白后非空的行，内部空白压成一个空格，取前 `TITLE_MAX_CHARS` 个字符。
pub(crate) fn title_from_brief(brief: &str) -> Option<String> {
    let line = brief.lines().map(str::trim).find(|l| !l.is_empty())?;
    let squeezed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(squeezed.chars().take(TITLE_MAX_CHARS).collect())
}

pub fn valid_executor(e: &str) -> bool {
    e == "claude"
        || e == "codex"
        || e.strip_prefix("acp:").is_some_and(|id| {
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
                && !id.contains("..")
        })
}

pub fn get(conn: &Connection, id: &str) -> Result<Session> {
    crewd_store::sessions::get(conn, id)?
        .ok_or_else(|| CollabError::NotFound(format!("没有 session {id}")))
}

pub fn list(conn: &Connection, crew: Option<&str>) -> Result<Vec<Session>> {
    Ok(match crew {
        Some(c) => crewd_store::sessions::list_for_crew(conn, c)?,
        None => crewd_store::sessions::list_all(conn)?,
    })
}

pub fn crew_of(conn: &Connection, id: &str) -> Result<String> {
    Ok(get(conn, id)?.crew_id)
}

/// 此刻的角色：机长以机组的机长指针为准（role 列漂移时不说错话）；指针之外，admin 如实报 admin，其余都是 worker。
pub fn role_of(conn: &Connection, id: &str) -> Result<Role> {
    let s = get(conn, id)?;
    Ok(
        if crews::captain_of(conn, &s.crew_id)?.as_deref() == Some(id) {
            Role::Captain
        } else if s.role == Role::Admin {
            Role::Admin
        } else {
            Role::Worker
        },
    )
}

/// 给人 / agent 看的名字：显示名 → 标题 → id。
pub fn display_name(conn: &Connection, id: &str) -> Result<String> {
    Ok(name_of(&get(conn, id)?))
}

/// 显示名的回退链（显示名 → 标题 → id）的唯一一处；`directory` 也用它。
pub(crate) fn name_of(s: &Session) -> String {
    s.display_name
        .clone()
        .or_else(|| s.title.clone())
        .unwrap_or_else(|| s.id.clone())
}

pub fn running_desired(conn: &Connection) -> Result<Vec<Session>> {
    Ok(list(conn, None)?
        .into_iter()
        .filter(|s| s.desired == Desired::Running)
        .collect())
}

pub fn register(tx: &mut Tx<'_>, n: NewSession) -> Result<Session> {
    let crew = crews::get(tx.conn(), &n.crew_id)?;
    if !valid_executor(&n.executor) {
        return Err(CollabError::Invalid(format!(
            "执行器 {:?} 不认识（claude / codex / acp:<id>）",
            n.executor
        )));
    }
    let title = match n.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) if t.chars().count() <= TITLE_MAX_CHARS => t.to_string(),
        Some(t) => {
            return Err(CollabError::Invalid(format!(
                "session 标题最长 {TITLE_MAX_CHARS} 个字（现在 {} 个）；写短一点，细节放进 brief",
                t.chars().count()
            )));
        }
        None => n
            .brief
            .as_deref()
            .and_then(title_from_brief)
            .ok_or_else(|| {
                CollabError::Invalid(format!(
                    "session 要有标题（不超过 {TITLE_MAX_CHARS} 个字）或 brief；两者都没有就没法给它起名"
                ))
            })?,
    };
    crate::prompts::refuse_sender_label("标题", &title)?;
    if let Some(v) = &n.display_name {
        crate::prompts::refuse_sender_label("显示名", v)?;
    }
    if n.role == Role::Captain && crews::captain_of(tx.conn(), &crew.id)?.is_some() {
        return Err(CollabError::Conflict(format!(
            "机组 {} 已有机长；换机长用 set_captain",
            crew.id
        )));
    }
    let conn = tx.conn();
    let ext = if n.role == Role::Worker {
        Some(crews::alloc_ext(conn, &crew.id)?)
    } else {
        None
    };
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    let s = Session {
        id: format!("{}-{}", n.role.as_str(), &uuid[..12]),
        crew_id: crew.id.clone(),
        role: n.role,
        executor: n.executor,
        display_name: n.display_name,
        working_directory: n.working_directory.or(crew.working_directory.clone()),
        model: n.model,
        effort: n.effort,
        updated_at: tx.now_ts(),
        title: Some(title),
        brief: n.brief,
        desired: Desired::Stopped,
        ext,
        worktree: n.worktree,
        ..Default::default()
    };
    crewd_store::sessions::upsert(conn, &s, None)?;
    if n.role == Role::Captain {
        crews::point_captain_at(tx, &crew.id, &s.id)?;
    }
    let lobby = crewd_store::channels::lobby_id(&crew.id);
    cursors::pin_to_end(tx, &s.id, &lobby)?;
    // 大厅的成员是隐式的（= 本机组全体 session）：多了一个人，大厅就变了（终审 M1）。
    tx.note(Change::ChannelChanged { channel_id: lobby });
    tx.note(Change::SessionChanged {
        session_id: s.id.clone(),
    });
    tx.note(Change::CrewChanged { crew_id: crew.id });
    get(tx.conn(), &s.id)
}

fn update_one(
    tx: &mut Tx<'_>,
    id: &str,
    sql: &str,
    p: impl rusqlite::Params,
    notify: bool,
) -> Result<()> {
    if tx.conn().execute(sql, p)? == 0 {
        return Err(CollabError::NotFound(format!("没有 session {id}")));
    }
    if notify {
        tx.note(Change::SessionChanged {
            session_id: id.into(),
        });
    }
    Ok(())
}

pub fn set_desired(tx: &mut Tx<'_>, id: &str, d: Desired) -> Result<()> {
    let now = tx.now_ts();
    update_one(
        tx,
        id,
        "UPDATE sessions SET desired=?2, updated_at=?3 WHERE id=?1",
        params![id, d.as_str(), now],
        true,
    )
}

/// 干活 / 空闲边沿（M3c 收到 `SessionEvent::Working` 时调）：记 was_working 与最后活跃时间。
pub fn set_working(tx: &mut Tx<'_>, id: &str, working: bool) -> Result<()> {
    let now = tx.now_ts();
    update_one(
        tx,
        id,
        "UPDATE sessions SET was_working=?2, last_active_at=?3 WHERE id=?1",
        params![id, working, now],
        true,
    )
}

/// 恢复失败（M4b 的 `reassign_lost` 只认这一列，§19）；`false` = 清掉。
pub fn set_resume_failed(tx: &mut Tx<'_>, id: &str, failed: bool) -> Result<()> {
    let at = failed.then(|| tx.now_ts());
    update_one(
        tx,
        id,
        "UPDATE sessions SET resume_failed_at=?2 WHERE id=?1",
        params![id, at],
        true,
    )
}

pub fn set_awaiting_question(tx: &mut Tx<'_>, id: &str, q: Option<String>) -> Result<()> {
    update_one(
        tx,
        id,
        "UPDATE sessions SET awaiting_question=?2 WHERE id=?1",
        params![id, q],
        true,
    )
}

pub fn set_profile(
    tx: &mut Tx<'_>,
    id: &str,
    model: Option<String>,
    effort: Option<String>,
) -> Result<()> {
    let now = tx.now_ts();
    update_one(
        tx,
        id,
        "UPDATE sessions SET model=COALESCE(?2, model), effort=COALESCE(?3, effort), updated_at=?4 WHERE id=?1",
        params![id, model, effort, now],
        true,
    )
}

/// 最近一次发言的时间（只是数据，**不算产出**）：只由 `messages::post` 在 session 发言时调。
pub(crate) fn note_output(tx: &mut Tx<'_>, id: &str) -> Result<()> {
    let now = tx.now_ts();
    update_one(
        tx,
        id,
        "UPDATE sessions SET last_output_at=?2 WHERE id=?1",
        params![id, now],
        false,
    )
}

/// 类型定义在 lib.rs（`Change::Progress` 要用，Task 5 先于本文件）。
pub use crate::ProgressKind;

/// 记一次产出：写 `last_progress_at`，发 `Change::Progress`（只走内部通道，M3c 的熔断订阅）。高频，不发 `SessionChanged`。
pub fn note_progress(tx: &mut Tx<'_>, id: &str, kind: ProgressKind) -> Result<()> {
    let now = tx.now_ts();
    update_one(
        tx,
        id,
        "UPDATE sessions SET last_progress_at=?2 WHERE id=?1",
        params![id, now],
        false,
    )?;
    tx.note(Change::Progress {
        session_id: id.into(),
        kind,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, count, err, ok, store};

    fn worker(crew: &str) -> NewSession {
        NewSession {
            crew_id: crew.into(),
            role: Role::Worker,
            executor: "claude".into(),
            title: Some("修滚动".into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_session_without_a_title_gets_one_from_its_brief() {
        let mut s = store();
        let got = ok(&mut s, |tx| {
            register(
                tx,
                NewSession {
                    title: None,
                    brief: Some("\n  一二三四五六七八九十甲乙丙丁戊己庚辛壬癸\n第二行".into()),
                    ..worker("crew-a")
                },
            )
        });
        assert_eq!(
            got.title.as_deref(),
            Some("一二三四五六七八九十甲乙丙丁戊己庚辛")
        );
    }

    #[test]
    fn a_session_needs_a_short_title_or_a_brief() {
        let mut s = store();
        assert!(matches!(
            err(&mut s, |tx| register(
                tx,
                NewSession {
                    title: None,
                    brief: None,
                    ..worker("crew-a")
                }
            )),
            CollabError::Invalid(_)
        ));
        assert!(
            matches!(
                err(&mut s, |tx| register(
                    tx,
                    NewSession {
                        title: Some("一二三四五六七八九十甲乙丙丁戊己庚辛壬".into()),
                        ..worker("crew-a")
                    }
                )),
                CollabError::Invalid(_)
            ),
            "19 字超长"
        );
        let ok18 = ok(&mut s, |tx| {
            register(
                tx,
                NewSession {
                    title: Some("  一二三四五六七八九十甲乙丙丁戊己庚辛  ".into()),
                    ..worker("crew-a")
                },
            )
        });
        assert_eq!(
            ok18.title.as_deref(),
            Some("一二三四五六七八九十甲乙丙丁戊己庚辛")
        );
    }

    #[test]
    fn register_assigns_extensions_that_are_never_reused() {
        let mut s = store();
        let a = ok(&mut s, |tx| register(tx, worker("crew-a")));
        assert_eq!(a.ext, Some(4));
        assert_eq!(a.desired, Desired::Stopped);
        assert_eq!(a.working_directory.as_deref(), Some("/tmp/example"));
        s.conn()
            .execute("DELETE FROM sessions WHERE id=?1", [&a.id])
            .unwrap();
        let b = ok(&mut s, |tx| register(tx, worker("crew-a")));
        assert_eq!(b.ext, Some(5));
        assert!(b.id.starts_with("worker-"));
    }

    #[test]
    fn a_crew_has_at_most_one_captain_via_register() {
        let mut s = store();
        let e = err(&mut s, |tx| {
            register(
                tx,
                NewSession {
                    role: Role::Captain,
                    ..worker("crew-a")
                },
            )
        });
        assert!(matches!(e, CollabError::Conflict(_)), "{e}");
        let c = ok(&mut s, |tx| {
            let crew = crate::crews::create(
                tx,
                crate::crews::NewCrew {
                    title: "空机组".into(),
                    working_directory: "/tmp/example".into(),
                    ..Default::default()
                },
            )?;
            register(
                tx,
                NewSession {
                    role: Role::Captain,
                    ..worker(&crew.id)
                },
            )
        });
        assert_eq!(c.ext, None, "机长分机恒为 1，不存");
        assert_eq!(
            crate::crews::captain_of(s.conn(), &c.crew_id)
                .unwrap()
                .as_deref(),
            Some(c.id.as_str())
        );
    }

    #[test]
    fn executor_vocabulary_is_checked() {
        let mut s = store();
        assert!(matches!(
            err(&mut s, |tx| register(
                tx,
                NewSession {
                    executor: "terminal".into(),
                    ..worker("crew-a")
                }
            )),
            CollabError::Invalid(_)
        ));
        ok(&mut s, |tx| {
            register(
                tx,
                NewSession {
                    executor: "acp:opencode".into(),
                    ..worker("crew-a")
                },
            )
        });
        assert!(!valid_executor("acp:"));
        assert!(!valid_executor("acp:../x"));
    }

    #[test]
    fn a_new_session_starts_with_no_unread() {
        let mut s = store();
        crewd_store::messages::insert(
            s.conn(),
            &crewd_proto::Message {
                id: "old".into(),
                channel_id: "crew-a/lobby".into(),
                text: "历史".into(),
                created_at: "t".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        let a = ok(&mut s, |tx| register(tx, worker("crew-a")));
        let max = count(&s, "SELECT MAX(seq) FROM messages");
        assert_eq!(
            crate::cursors::cursor(s.conn(), &a.id, "crew-a/lobby").unwrap(),
            Some(max)
        );
    }

    #[test]
    fn progress_and_output_are_separate_columns() {
        let mut s = store();
        let ((), ch) = crate::testkit::ok_at(&mut s, NOW, |tx| note_output(tx, "w-a1"));
        assert!(ch.is_empty(), "note_output 不发事件");
        let a = get(s.conn(), "w-a1").unwrap();
        assert!(a.last_output_at.is_some());
        assert_eq!(a.last_progress_at, None, "发言不算产出（C3）");
        let ((), _) = crate::testkit::ok_at(&mut s, NOW + 5_000, |tx| {
            note_progress(tx, "w-a1", ProgressKind::WorkItem)
        });
        let b = get(s.conn(), "w-a1").unwrap();
        assert_eq!(b.last_output_at, a.last_output_at, "产出不动发言时间");
        assert_eq!(
            b.last_progress_at.as_deref(),
            Some(crewd_store::ts_from_ms(NOW + 5_000).as_str())
        );
        assert!(matches!(
            err(&mut s, |tx| note_progress(
                tx,
                "ghost",
                ProgressKind::ToolCall
            )),
            CollabError::NotFound(_)
        ));
    }

    #[test]
    fn lifecycle_setters_write_their_columns() {
        let mut s = store();
        ok(&mut s, |tx| set_desired(tx, "w-a1", Desired::Running));
        ok(&mut s, |tx| set_working(tx, "w-a1", true));
        ok(&mut s, |tx| set_resume_failed(tx, "w-a2", true));
        ok(&mut s, |tx| {
            set_awaiting_question(tx, "w-a1", Some("要不要接着做 B？".into()))
        });
        ok(&mut s, |tx| note_output(tx, "w-a1"));
        let ((), ch) = crate::testkit::ok_at(&mut s, NOW, |tx| {
            note_progress(tx, "w-a1", ProgressKind::ToolCall)
        });
        assert_eq!(
            ch,
            [Change::Progress {
                session_id: "w-a1".into(),
                kind: ProgressKind::ToolCall
            }]
        );
        let a = get(s.conn(), "w-a1").unwrap();
        assert_eq!(a.desired, Desired::Running);
        assert!(a.was_working);
        assert_eq!(
            a.last_active_at.as_deref(),
            Some(crewd_store::ts_from_ms(NOW).as_str())
        );
        assert_eq!(a.last_output_at, a.last_active_at);
        assert_eq!(a.last_progress_at, a.last_active_at, "产出单独一列（C3）");
        assert_eq!(a.awaiting_question.as_deref(), Some("要不要接着做 B？"));
        assert!(get(s.conn(), "w-a2").unwrap().resume_failed_at.is_some());
        ok(&mut s, |tx| set_resume_failed(tx, "w-a2", false));
        assert!(get(s.conn(), "w-a2").unwrap().resume_failed_at.is_none());
        assert_eq!(
            running_desired(s.conn())
                .unwrap()
                .into_iter()
                .map(|x| x.id)
                .collect::<Vec<_>>(),
            ["w-a1"]
        );
        assert!(matches!(
            err(&mut s, |tx| set_desired(tx, "ghost", Desired::Running)),
            CollabError::NotFound(_)
        ));
    }

    #[test]
    fn registering_a_captain_pins_the_root_human_channel_like_set_captain() {
        let mut s = store();
        let crew = ok(&mut s, |tx| {
            crate::crews::create(
                tx,
                crate::crews::NewCrew {
                    title: "空机组".into(),
                    working_directory: "/tmp/example".into(),
                    ..Default::default()
                },
            )
        });
        let human = crewd_store::channels::human_id(&crew.id);
        s.conn()
            .execute(
                "INSERT INTO channels(id,crew_id,kind,name,created_by,created_at) VALUES(?1,?2,'human','','system','2026-09-01T00:00:00.000Z')",
                [&human, &crew.id],
            )
            .unwrap();
        crewd_store::messages::insert(
            s.conn(),
            &crewd_proto::Message {
                id: "m-h".into(),
                channel_id: human.clone(),
                text: "示例消息".into(),
                created_at: "2026-09-21T00:00:00.000Z".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        let end = crewd_store::messages::max_seq(s.conn(), &human).unwrap();
        let (c, ch) = crate::testkit::ok_at(&mut s, NOW, |tx| {
            register(
                tx,
                NewSession {
                    role: Role::Captain,
                    ..worker(&crew.id)
                },
            )
        });
        assert_eq!(
            crate::cursors::cursor(s.conn(), &c.id, &human).unwrap(),
            Some(end)
        );
        assert!(ch.contains(&Change::ChannelChanged { channel_id: human }));
    }

    #[test]
    fn role_of_reports_admin_and_follows_the_captain_pointer() {
        let mut s = store();
        let admin = ok(&mut s, |tx| {
            register(
                tx,
                NewSession {
                    role: Role::Admin,
                    ..worker("crew-a")
                },
            )
        });
        assert_eq!(role_of(s.conn(), &admin.id).unwrap(), Role::Admin);
        assert_eq!(role_of(s.conn(), "cap-a").unwrap(), Role::Captain);
        assert_eq!(role_of(s.conn(), "w-a1").unwrap(), Role::Worker);
        // role 列漂移（还写着 captain 但指针不指向它）时以指针为准。
        s.conn()
            .execute("UPDATE sessions SET role='captain' WHERE id='w-a1'", [])
            .unwrap();
        assert_eq!(role_of(s.conn(), "w-a1").unwrap(), Role::Worker);
    }

    #[test]
    fn set_profile_updates_given_fields_and_keeps_the_rest() {
        let mut s = store();
        let ((), ch) = crate::testkit::ok_at(&mut s, NOW, |tx| {
            set_profile(tx, "w-a1", Some("model-a".into()), Some("high".into()))
        });
        assert_eq!(
            ch,
            [Change::SessionChanged {
                session_id: "w-a1".into()
            }]
        );
        ok(&mut s, |tx| {
            set_profile(tx, "w-a1", None, Some("low".into()))
        });
        let a = get(s.conn(), "w-a1").unwrap();
        assert_eq!(
            (a.model.as_deref(), a.effort.as_deref()),
            (Some("model-a"), Some("low")),
            "None = 保留旧值"
        );
        ok(&mut s, |tx| {
            set_profile(tx, "w-a1", Some("model-b".into()), None)
        });
        let a = get(s.conn(), "w-a1").unwrap();
        assert_eq!(
            (a.model.as_deref(), a.effort.as_deref()),
            (Some("model-b"), Some("low"))
        );
        assert_eq!(a.updated_at, crewd_store::ts_from_ms(NOW));
        assert!(matches!(
            err(&mut s, |tx| set_profile(tx, "ghost", None, None)),
            CollabError::NotFound(_)
        ));
    }

    /// 终审 I5：session 的标题 / 显示名不能是「人类」「crewd」这类发言者标签（任一语言，去空白、压换行后比）。
    #[test]
    fn a_session_cannot_be_named_like_the_human_or_crewd() {
        let mut s = store();
        for bad in [
            "人类",
            " Human\n",
            "crewd",
            "human",
            "人\u{200B}类",
            "\u{FEFF}Hu\u{200C}m\u{200D}an\u{2060}",
            "人\u{202E}类",
            "\u{200E}crewd\u{200F}",
        ] {
            for n in [
                NewSession {
                    title: Some(bad.into()),
                    ..worker("crew-a")
                },
                NewSession {
                    display_name: Some(bad.into()),
                    ..worker("crew-a")
                },
            ] {
                assert!(
                    matches!(err(&mut s, |tx| register(tx, n)), CollabError::Invalid(_)),
                    "{bad:?} 不能当 session 名"
                );
            }
        }
        ok(&mut s, |tx| {
            register(
                tx,
                NewSession {
                    display_name: Some("人类学家".into()),
                    ..worker("crew-a")
                },
            )
        });
    }

    /// 终审 M1：注册 = 大厅多了一个隐式成员，大厅要发 ChannelChanged（客户端的成员列表才会刷新）。
    #[test]
    fn registering_notes_the_lobby_whose_implicit_members_changed() {
        let mut s = store();
        let (_, ch) = crate::testkit::ok_at(&mut s, NOW, |tx| register(tx, worker("crew-a")));
        assert!(
            ch.contains(&Change::ChannelChanged {
                channel_id: "crew-a/lobby".into()
            }),
            "{ch:?}"
        );
    }
}
