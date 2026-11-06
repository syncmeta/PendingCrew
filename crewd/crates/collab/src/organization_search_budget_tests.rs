//! Real public message/search calls under a wide deterministic SQLite VM budget.
use super::*;
use std::ffi::{c_int, c_void};
const STEP_INTERVAL: i32 = 1000;
const VM_BUDGET: usize = 8_000_000;
struct Counter {
    steps: usize,
    limit: usize,
}
struct VmBudget<'a> {
    connection: &'a rusqlite::Connection,
    counter: Box<Counter>,
}
unsafe extern "C" fn progress(context: *mut c_void) -> c_int {
    // SQLite calls synchronously while this fixture-owned Box remains alive.
    let counter = unsafe { &mut *context.cast::<Counter>() };
    counter.steps = counter.steps.saturating_add(STEP_INTERVAL as usize);
    if counter.steps >= counter.limit { 1 } else { 0 }
}
impl<'a> VmBudget<'a> {
    fn install(connection: &'a rusqlite::Connection) -> Self {
        let mut guard = Self {
            connection,
            counter: Box::new(Counter {
                steps: 0,
                limit: VM_BUDGET,
            }),
        };
        // This newly owned test Store has no prior progress callback. The Box's
        // allocation is stable when the guard moves; Drop unregisters first.
        unsafe {
            rusqlite::ffi::sqlite3_progress_handler(
                connection.handle(),
                STEP_INTERVAL,
                Some(progress),
                (&mut *guard.counter as *mut Counter).cast(),
            )
        };
        guard
    }
}
impl Drop for VmBudget<'_> {
    fn drop(&mut self) {
        // Connection is borrowed through guard lifetime; never leave a callback
        // pointing into the fixture after success, error or unwinding.
        unsafe {
            rusqlite::ffi::sqlite3_progress_handler(
                self.connection.handle(),
                0,
                None,
                std::ptr::null_mut(),
            )
        };
    }
}
fn corpus() -> Store {
    let mut s = fixture();
    ok(&mut s, |tx| {
        for i in 0..1000 {
            crate::messages::post(
                tx,
                Sender::Human,
                crate::messages::NewPost {
                    channel_id: "crew-a/lobby".into(),
                    text: format!("\u{68c0}\u{7d22}\u{6837}\u{672c} deterministic corpus {i}"),
                    ..Default::default()
                },
            )?;
        }
        tx.organization_search_budget = 0;
        Ok(())
    });
    ready(&mut s);
    s
}
#[test]
fn literal_long_and_short_search_finish_under_vm_budget_and_do_not_mutate() {
    let s = corpus();
    let before = crate::testkit::database_snapshot(&s);
    for term in ["\u{68c0}\u{7d22}\u{6837}\u{672c}", "\u{68c0}\u{7d22}"] {
        let mut q = query(term);
        q.r#in = Domain::Messages;
        q.channel_id = Some("crew-a/lobby".into());
        q.limit = 200;
        let guard = VmBudget::install(s.conn());
        let result = run_search(s.conn(), HUMAN, q, NOW + 100_000);
        let steps = guard.counter.steps;
        drop(guard);
        let page = result.unwrap_or_else(|e| {
            panic!("public search exceeded bounded VM work after {steps} steps: {e:?}")
        });
        assert_eq!(page.index_state, IndexState::Ready);
        assert_eq!(page.items.len(), 128);
        assert!(page.next_cursor.is_some());
        assert!(steps < VM_BUDGET);
        assert!(
            page.items
                .iter()
                .all(|h| matches!(h.target, v2::TargetRef::Message { .. }))
        );
    }
    assert_eq!(crate::testkit::database_snapshot(&s), before);
}
