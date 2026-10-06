//! 世界观的上下文与渲染签名（spec §22.5：M3a 定签名，M3b 写 prompts/world-model.*.md 与 `render` 的正文）。
//! 变量照 `Sources/Stores/LocalSessionWorldModel.swift:73-90`；M3b 可以往 `WorldCtx` 加字段（文件归 M3b）。
use crate::{Locale, Result, channels, crews, sessions};
use crewd_proto::Role;
use rusqlite::Connection;
use std::sync::LazyLock;

use crate::prompts::{PromptFile, parse};

static TOOLS_ZH: LazyLock<PromptFile> = LazyLock::new(|| {
    parse("tools.zh", include_str!("../prompts/tools.zh.md")).unwrap_or_else(|e| panic!("{e}"))
});
static TOOLS_EN: LazyLock<PromptFile> = LazyLock::new(|| {
    parse("tools.en", include_str!("../prompts/tools.en.md")).unwrap_or_else(|e| panic!("{e}"))
});

/// `prompts/tools.{zh,en}.md`：工具描述、参数说明、回执、代发稿。整个 workspace 只在这里加载一次
/// （collab 的 `turn_trace` 取 `trace.*`，server 的 `agent::text` 取其余）。
pub fn tools_file(l: Locale) -> &'static PromptFile {
    match l {
        Locale::Zh => &TOOLS_ZH,
        Locale::En => &TOOLS_EN,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorldCtx {
    pub locale: Locale,
    pub session_id: String,
    pub role: Role,
    pub executor: String,
    pub brief: String,
    pub crew_id: String,
    pub crew_title: String,
    pub working_directory: String,
    pub captain_name: Option<String>,
    pub parent_titles: Vec<String>,
    pub child_titles: Vec<String>,
    /// 它在的群（id）。
    pub channels: Vec<String>,
    /// 找人类该发到哪：人类正在注视的群 + 根机组人类群（§17）。
    pub human_watching: Vec<String>,
}

pub fn context_for(conn: &Connection, session: &str, locale: Locale) -> Result<WorldCtx> {
    let s = sessions::get(conn, session)?;
    let crew = crews::get(conn, &s.crew_id)?;
    let titles = |ids: Vec<String>| -> Result<Vec<String>> {
        ids.iter().map(|i| Ok(crews::get(conn, i)?.title)).collect()
    };
    let mut watching: Vec<String> = {
        let mut st =
            conn.prepare("SELECT channel_id FROM channel_watch ORDER BY since, channel_id")?;
        st.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    let root_human = crewd_store::channels::human_id(&crews::root_of(conn, &crew.id)?);
    if !watching.contains(&root_human) {
        watching.push(root_human);
    }
    Ok(WorldCtx {
        locale,
        session_id: s.id.clone(),
        role: sessions::role_of(conn, &s.id)?,
        executor: s.executor.clone(),
        brief: s.brief.clone().unwrap_or_default(),
        crew_id: crew.id.clone(),
        crew_title: crew.title.clone(),
        working_directory: s
            .working_directory
            .clone()
            .or(crew.working_directory.clone())
            .unwrap_or_default(),
        captain_name: crews::captain_of(conn, &crew.id)?
            .map(|c| sessions::display_name(conn, &c))
            .transpose()?,
        parent_titles: titles(crews::parents(conn, &crew.id)?)?,
        child_titles: titles(crews::children(conn, &crew.id)?)?,
        channels: channels::channels_of_session(conn, &s.id)?
            .into_iter()
            .map(|c| c.id)
            .collect(),
        human_watching: watching,
    })
}

/// M3b 实现（读 prompts/world-model.{zh,en}.md）。M3a 返回空串：claude 的 `--append-system-prompt-file` 得到空文件。
/// M3 集成验收必须核「世界观非空」（见计划 Task 15 交付记录的遗留）。
pub fn render(_ctx: &WorldCtx) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{ok, store};

    #[test]
    fn context_collects_crew_lineage_channels_and_where_to_find_the_human() {
        let mut s = store();
        ok(&mut s, |tx| {
            crate::channels::watch(tx, "crew-b/lobby", true)
        });
        let c = context_for(s.conn(), "w-b1", Locale::Zh).unwrap();
        assert_eq!(c.crew_title, "机组乙");
        assert_eq!(c.role, Role::Worker);
        assert_eq!(c.captain_name.as_deref(), Some("名字-cap-b"));
        assert_eq!(c.parent_titles, ["机组甲"]);
        assert_eq!(c.channels, ["crew-b/lobby"]);
        assert_eq!(c.human_watching, ["crew-b/lobby", "crew-a/human"]);
    }

    /// M3b 实现 `render` 时删掉这条。
    #[test]
    fn render_is_a_stub_until_m3b() {
        let s = store();
        assert_eq!(
            render(&context_for(s.conn(), "w-a1", Locale::Zh).unwrap()),
            ""
        );
    }
}
