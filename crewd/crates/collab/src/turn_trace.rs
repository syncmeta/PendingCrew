//! 回合收尾停在没叫到人的问句上 → 代发（spec §22.3，移植 `Sources/Mcp/SessionTurnTrace.swift`）。
//! M3a 只定签名；M3b 实现（按 prompt_id 去重、永不阻塞、写 `sessions.awaiting_question`）。
use crate::{Result, Tx};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEnd {
    /// 不需要代发。
    Nothing,
    /// 代发了一条（worker → 大厅 @机长；机长 → 根机组人类群 @人）。
    Posted { message_id: String },
    /// 这一轮已经 @ 过能答的人（机长 / 人）。
    AlreadyCalled,
    /// 同一 prompt_id 已经处理过（hook 重放）。
    Replayed,
    /// 不知道这一轮叫没叫过人（从没记过账，如重启后第一轮）：宁可不代发（`SessionTurnTrace.swift:42-43`）。
    Unknown,
}

pub fn on_turn_end(
    _tx: &mut Tx<'_>,
    _session: &str,
    _prompt_id: &str,
    _last_text: &str,
) -> Result<TurnEnd> {
    Ok(TurnEnd::Nothing)
}
