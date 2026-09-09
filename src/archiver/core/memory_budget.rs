use std::sync::atomic::{AtomicI64, Ordering};


pub const MEMORY_LIMIT_IN_BYTES: i64 = 8 * 1024_i64.pow(3); // 8GB
pub static MEMORY_BUDGET_IN_BYTES: AtomicI64 = AtomicI64::new(MEMORY_LIMIT_IN_BYTES);


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

        let (color, symbol) =
        match (is_filled_now, was_filled) {
            (true, false)  => ("\x1b[32m", '#'), // зелёный - память увеличилась
            (false, true)  => ("\x1b[36m", '#'), // голубой - память уменьшилась
            (true, true)   => ("\x1b[0m",  '#'),
            (false, false) => ("\x1b[0m", ' '),
        };
        bar.push_str(color);
        bar.push(symbol);
    }
    bar.push_str("\x1b[0m]"); // сброс цвета

    let used_gb = used;
    let total_gb = total;
    let info = format!("{:.1}% ({:.2} B / {:.2} B)\n", percent, used_gb, total_gb);

    println!("{} {}", bar, info);

    PREV_FILLED.store(filled, Ordering::Relaxed);
}
