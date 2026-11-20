//! 计算模式（`=`）：算打的式子（fend：四则、乘方、百分比、单位换算）。算得太久就停下，打字不卡。

use std::time::{Duration, Instant};

/// 一个式子最多算这么久：`10^10^10` 这种会算很久，到点就当算不出来。
const MOST_TIME: Duration = Duration::from_millis(50);

/// 结果；算不出来是 None（fend 的原因是英文的，不给用户看）。
pub(crate) fn calculate(expression: &str) -> Option<String> {
    if expression.is_empty() {
        return None;
    }
    let mut context = fend_core::Context::new();
    let deadline = Deadline(Instant::now() + MOST_TIME);
    let result = fend_core::evaluate_with_interrupt(expression, &mut context, &deadline).ok()?;
    let answer = result.get_main_result().trim();
    (!answer.is_empty()).then(|| answer.to_owned())
}

struct Deadline(Instant);

impl fend_core::Interrupt for Deadline {
    fn should_interrupt(&self) -> bool {
        Instant::now() >= self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_and_units_are_worked_out() {
        assert_eq!(calculate("1+2*3").as_deref(), Some("7"));
        assert_eq!(calculate("2^10").as_deref(), Some("1024"));
        assert_eq!(calculate("1/4").as_deref(), Some("0.25"));
        assert!(calculate("1 GiB to MiB").unwrap().starts_with("1024"));
        assert_eq!(calculate("1+"), None);
        assert_eq!(calculate(""), None);
    }

    #[test]
    fn a_huge_calculation_stops_in_time() {
        let started = Instant::now();
        let _ = calculate("10^10^10^10");
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
