//! 字节进度分数：文件操作进度条、传输占位和远程预览下载面板共用同一份算法，
//! 不在各处各留一份。

/// 进行中的任务永远到不了 1.0：满格只属于已经结算完成的任务，
/// 否则最后一个字节写完到结算之间进度条会提前显示完成。
pub const ACTIVE_PROGRESS_LIMIT: f32 = 0.999;

pub fn active_byte_fraction(completed_bytes: u64, total_bytes: u64) -> Option<f32> {
    if total_bytes == 0 {
        return None;
    }
    Some(
        ((completed_bytes.min(total_bytes) as f64 / total_bytes as f64) as f32)
            .min(ACTIVE_PROGRESS_LIMIT),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_byte_fraction_is_unknown_without_a_denominator_and_never_reaches_terminal() {
        assert_eq!(active_byte_fraction(0, 0), None);
        assert_eq!(active_byte_fraction(250, 1_000), Some(0.25));
        assert_eq!(
            active_byte_fraction(1_000, 1_000),
            Some(ACTIVE_PROGRESS_LIMIT)
        );
    }
}
