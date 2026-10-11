//! helper 身份令牌（spec §22.3）：M3c 起会话时 `mint`，M3b 的 `agent.attach` 用 `verify`。
//! 只在内存里：crewd 重启时旧进程已经跟着死了。用途是防串号（配错、复用旧配置），不是挡 agent 本身。
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct TokenTable {
    inner: Mutex<HashMap<String, String>>,
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl TokenTable {
    /// Fixture cardinality only; never exposes token values or a production RPC.
    #[cfg(test)]
    pub(crate) fn test_len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// 给 session 发一个新令牌（替换旧的）。64 个十六进制字符。
    pub fn mint(&self, session: &str) -> String {
        let t = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session.to_string(), t.clone());
        t
    }

    pub fn verify(&self, session: &str, token: &str) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session)
            .is_some_and(|t| ct_eq(t.as_bytes(), token.as_bytes()))
    }

    pub fn revoke(&self, session: &str) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_per_session_and_replaceable() {
        let t = TokenTable::default();
        let a = t.mint("w-1");
        assert_eq!(a.len(), 64);
        assert!(t.verify("w-1", &a));
        assert!(!t.verify("w-2", &a), "别的 session 不能用");
        assert!(!t.verify("w-1", "x"));
        let b = t.mint("w-1");
        assert!(!t.verify("w-1", &a), "重新发放后旧的作废");
        assert!(t.verify("w-1", &b));
        t.revoke("w-1");
        assert!(!t.verify("w-1", &b));
    }
    #[test]
    fn poisoned_valid_table_retains_tokens_and_all_operations_recover() {
        let table = TokenTable::default();
        let original = table.mint("existing");
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = table.inner.lock().unwrap();
            panic!("owned poison fault");
        }));
        assert!(poisoned.is_err());
        assert!(table.verify("existing", &original));
        assert!(!table.verify("other", &original));
        assert!(!table.verify("existing", "wrong"));
        let fresh = table.mint("new");
        assert!(table.verify("new", &fresh));
        assert_eq!(table.test_len(), 2);
        let replacement = table.mint("existing");
        assert!(!table.verify("existing", &original));
        assert!(table.verify("existing", &replacement));
        table.revoke("existing");
        assert!(!table.verify("existing", &replacement));
        assert!(table.verify("new", &fresh));
        assert_eq!(table.test_len(), 1);
    }
}
