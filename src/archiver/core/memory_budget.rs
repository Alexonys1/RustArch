use std::sync::atomic::{AtomicI64, Ordering};

#[cfg(test)]
use std::sync::{Mutex, MutexGuard};

use owo_colors::OwoColorize;


pub const MEMORY_LIMIT_IN_BYTES: i64 = 6 * 1024_i64.pow(3); // 6GB
pub static MEMORY_BUDGET_IN_BYTES: AtomicI64 = AtomicI64::new(MEMORY_LIMIT_IN_BYTES);

const _: () = assert!(MEMORY_LIMIT_IN_BYTES > 0);

#[cfg(test)]
static MEMORY_BUDGET_TEST_LOCK: Mutex<()> = Mutex::new(());


#[cfg(test)]
pub(crate) struct TestMemoryBudget {
    original_budget: i64,
    _lock: MutexGuard<'static, ()>,
}


#[cfg(test)]
impl TestMemoryBudget {
    pub(crate) fn new(available_bytes: i64) -> Self {
        let lock = MEMORY_BUDGET_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let original_budget = MEMORY_BUDGET_IN_BYTES.swap(available_bytes, Ordering::AcqRel);

        Self {
            original_budget,
            _lock: lock,
        }
    }

    pub(crate) fn remaining(&self) -> i64 {
        MEMORY_BUDGET_IN_BYTES.load(Ordering::Acquire)
    }
}


#[cfg(test)]
impl Drop for TestMemoryBudget {
    fn drop(&mut self) {
        MEMORY_BUDGET_IN_BYTES.store(self.original_budget, Ordering::Release);
    }
}


// TODO: Его нужно сделать неизменяемым снаружи (interior mutability)?
pub struct BudgetGuard {
    busy_memory_in_bytes: i64,
}


impl BudgetGuard {
    pub fn try_grow(&mut self, additional_bytes: i64) -> bool {
        let mut current = MEMORY_BUDGET_IN_BYTES.load(Ordering::Acquire);

        loop {
            if current < additional_bytes {
                return false;
            }

            match MEMORY_BUDGET_IN_BYTES.compare_exchange_weak(
                current,
                current - additional_bytes,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.busy_memory_in_bytes += additional_bytes;
                    return true;
                }
                Err(actual) => current = actual,
            }
        }
    }

    /// Возвращает `realised_bytes` в глобальный бюджет
    /// и синхронно уменьшает локальный счётчик, чтобы
    /// Drop не вернул эти байты в бюджет повторно.
    pub fn release(&mut self, realised_bytes: i64) {
        self.busy_memory_in_bytes -= realised_bytes;
        MEMORY_BUDGET_IN_BYTES.fetch_add(realised_bytes, Ordering::AcqRel);
    }
}


impl Drop for BudgetGuard {
    fn drop(&mut self) {
        MEMORY_BUDGET_IN_BYTES.fetch_add(self.busy_memory_in_bytes, Ordering::AcqRel);
    }
}


impl Default for BudgetGuard {
    fn default() -> Self {
        Self { busy_memory_in_bytes: 0 }
    }
}


pub fn show_memory_bar() {
    use std::sync::atomic::{ AtomicUsize, Ordering };
    static PREV_FILLED: AtomicUsize = AtomicUsize::new(0);
    const BAR_WIDTH: usize = 50;

    let budget = MEMORY_BUDGET_IN_BYTES.load(Ordering::Relaxed);
    let used = MEMORY_LIMIT_IN_BYTES - budget;
    let total = MEMORY_LIMIT_IN_BYTES;
    let percent = (used as f64 / total as f64) * 100.0;
    let filled = (percent * BAR_WIDTH as f64 / 100.0).round() as usize;
    let filled = filled.min(BAR_WIDTH);

    let prev_filled = PREV_FILLED.load(Ordering::Relaxed);

    let mut bar = String::with_capacity(BAR_WIDTH * 10);
    bar.push('[');
    for i in 0..BAR_WIDTH {
        let is_filled_now = i < filled;
        let was_filled = i < prev_filled;

        let symbol = match (is_filled_now, was_filled) {
            (true, false) => "#".green().to_string(), // зелёный - память увеличилась
            (false, true) => "#".cyan().to_string(),  // голубой - память уменьшилась
            (true, true) => "#".to_string(),
            (false, false) => " ".to_string(),
        };
        bar.push_str(&symbol);
    }
    bar.push(']');

    let used_gb = used;
    let total_gb = total;
    let info = format!("{:.1}% ({:.2} B / {:.2} B)\n", percent, used_gb, total_gb);

    println!("{} {}", bar, info);

    PREV_FILLED.store(filled, Ordering::Relaxed);
}


#[cfg(test)]
mod tests {
    use super::{BudgetGuard, TestMemoryBudget};

    #[test]
    fn budget_guard_reserves_releases_and_returns_memory_on_drop() {
        let budget = TestMemoryBudget::new(10);
        let mut guard = BudgetGuard::default();

        assert!(guard.try_grow(6));
        assert_eq!(budget.remaining(), 4);

        guard.release(2);
        assert_eq!(budget.remaining(), 6);

        drop(guard);
        assert_eq!(budget.remaining(), 10);
    }

    #[test]
    fn budget_guard_rejects_growth_over_remaining_budget() {
        let budget = TestMemoryBudget::new(3);
        let mut guard = BudgetGuard::default();

        assert!(!guard.try_grow(4));
        assert_eq!(budget.remaining(), 3);
    }
}
