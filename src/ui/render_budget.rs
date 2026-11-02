use eframe::egui::{Context, Id, ViewportId};
use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard, TryLockError},
    time::{Duration, Instant},
};

const FRAME_WAIT: Duration = Duration::from_millis(2);
const PANE_WAIT: Duration = Duration::from_micros(500);

#[derive(Clone)]
struct Budget {
    frame: u64,
    visible: usize,
    used: Duration,
    panes: HashMap<Id, Duration>,
}

impl Budget {
    fn new(frame: u64) -> Self {
        Self {
            frame,
            visible: 1,
            used: Duration::ZERO,
            panes: HashMap::new(),
        }
    }

    fn allowance(&self, pane: Id) -> Duration {
        let share = (FRAME_WAIT / self.visible.min(u32::MAX as usize) as u32).min(PANE_WAIT);
        share
            .saturating_sub(self.panes.get(&pane).copied().unwrap_or_default())
            .min(FRAME_WAIT.saturating_sub(self.used))
    }

    fn charge(&mut self, pane: Id, elapsed: Duration) {
        self.used = self.used.saturating_add(elapsed);
        let used = self.panes.entry(pane).or_default();
        *used = used.saturating_add(elapsed);
    }
}

fn with_budget<R>(ctx: &Context, f: impl FnOnce(&mut Budget) -> R) -> R {
    // Immediate child windows render inside the root frame, including discard passes.
    let frame = ctx.cumulative_frame_nr_for(ViewportId::ROOT);
    ctx.data_mut(|data| {
        let budget = data
            .get_temp_mut_or_insert_with(Id::new("terminal render wait"), || Budget::new(frame));
        if budget.frame != frame {
            budget.frame = frame;
            budget.visible = 1;
            budget.used = Duration::ZERO;
            budget.panes.clear();
        }
        f(budget)
    })
}

pub(super) fn prepare(ctx: &Context, visible: usize) {
    with_budget(ctx, |budget| budget.visible = budget.visible.max(visible));
}

pub(super) fn lock<'a, T>(ctx: &Context, state: &'a Mutex<T>) -> Option<MutexGuard<'a, T>> {
    match state.try_lock() {
        Ok(guard) => return Some(guard),
        Err(TryLockError::Poisoned(_)) => return None,
        Err(TryLockError::WouldBlock) => {}
    }
    if ctx.input(|input| input.viewport().minimized.unwrap_or(false)) {
        return None;
    }
    let started = Instant::now();
    let pane = Id::new(("terminal grid", std::ptr::from_ref(state)));
    let allowance = with_budget(ctx, |budget| budget.allowance(pane));
    if allowance.is_zero() {
        return None;
    }
    let result = loop {
        if started.elapsed() >= allowance {
            break None;
        }
        std::thread::yield_now();
        match state.try_lock() {
            Ok(guard) => break Some(guard),
            Err(TryLockError::Poisoned(_)) => break None,
            Err(TryLockError::WouldBlock) => {}
        }
    };
    with_budget(ctx, |budget| budget.charge(pane, started.elapsed()));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_early_slow_pane_cannot_spend_later_panes_shares() {
        let mut budget = Budget::new(0);
        budget.visible = 16;
        for index in 0..16 {
            let pane = Id::new(index);
            assert_eq!(budget.allowance(pane), Duration::from_micros(125));
            budget.charge(pane, Duration::from_micros(125));
            assert_eq!(budget.allowance(pane), Duration::ZERO);
        }
        assert_eq!(budget.used, FRAME_WAIT);
        assert_eq!(budget.allowance(Id::new("new pane")), Duration::ZERO);
    }

    #[test]
    fn exhausted_wait_still_allows_uncontended_panes_and_recovers_next_frame() {
        let ctx = Context::default();
        let grid = Mutex::new(());
        let output = ctx.run(Default::default(), |ctx| {
            prepare(ctx, 16);
            with_budget(ctx, |budget| budget.charge(Id::new("slow"), FRAME_WAIT));
            assert!(lock(ctx, &grid).is_some());
            let held = grid.lock().unwrap();
            assert!(lock(ctx, &grid).is_none());
            drop(held);
            assert_eq!(with_budget(ctx, |budget| budget.used), FRAME_WAIT);
        });
        drop(output);
        let output = ctx.run(Default::default(), |ctx| {
            prepare(ctx, 1);
            assert_eq!(
                with_budget(ctx, |budget| budget.allowance(Id::new("new"))),
                PANE_WAIT
            );
            assert_eq!(with_budget(ctx, |budget| budget.used), Duration::ZERO);
        });
        drop(output);
    }

    #[test]
    fn discard_passes_do_not_refill_or_reassign_a_panes_wait() {
        let ctx = Context::default();
        let pane = Id::new("pane");
        let mut passes = 0;
        let output = ctx.run(Default::default(), |ctx| {
            prepare(ctx, 4);
            passes += 1;
            if passes == 1 {
                with_budget(ctx, |budget| budget.charge(pane, PANE_WAIT));
                ctx.request_discard("test repeated layout");
            } else {
                prepare(ctx, 1);
                assert_eq!(
                    with_budget(ctx, |budget| budget.allowance(pane)),
                    Duration::ZERO
                );
                assert_eq!(with_budget(ctx, |budget| budget.visible), 4);
                assert_eq!(
                    with_budget(ctx, |budget| budget.allowance(Id::new("later"))),
                    PANE_WAIT
                );
            }
        });
        drop(output);
        assert_eq!(passes, 2);
    }

    #[test]
    fn nested_windows_share_the_root_budget() {
        let ctx = Context::default();
        let child = ViewportId::from_hash_of("child");
        let output = ctx.run(Default::default(), |ctx| {
            prepare(ctx, 8);
            with_budget(ctx, |budget| {
                budget.charge(Id::new("root pane"), FRAME_WAIT)
            });
            let output = ctx.run(
                eframe::egui::RawInput {
                    viewport_id: child,
                    viewports: [(
                        child,
                        eframe::egui::ViewportInfo {
                            parent: Some(ViewportId::ROOT),
                            ..Default::default()
                        },
                    )]
                    .into_iter()
                    .collect(),
                    ..Default::default()
                },
                |ctx| {
                    prepare(ctx, 4);
                    assert_eq!(
                        with_budget(ctx, |budget| budget.allowance(Id::new("child pane"))),
                        Duration::ZERO
                    );
                },
            );
            drop(output);
        });
        drop(output);
    }

    #[test]
    fn minimized_windows_do_not_spend_retry_budget() {
        let ctx = Context::default();
        let grid = Mutex::new(());
        let held = grid.lock().unwrap();
        let mut input = eframe::egui::RawInput::default();
        input
            .viewports
            .get_mut(&ViewportId::ROOT)
            .unwrap()
            .minimized = Some(true);
        let output = ctx.run(input, |ctx| {
            prepare(ctx, 1);
            assert!(lock(ctx, &grid).is_none());
            assert_eq!(with_budget(ctx, |budget| budget.used), Duration::ZERO);
        });
        drop(output);
        drop(held);
    }

    #[test]
    fn scheduler_overshoot_is_charged_before_later_panes_retry() {
        let mut budget = Budget::new(0);
        budget.visible = 8;
        budget.charge(Id::new("preempted"), FRAME_WAIT + Duration::from_millis(1));
        assert_eq!(budget.allowance(Id::new("later")), Duration::ZERO);
    }
}
