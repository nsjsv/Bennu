//! 文案表：每条是（英文句式，中文句式），英文句式就是 `tr!` 的消息 ID。按界面区域
//! 分文件；同一句英文全表只能出现一次（`message_tests` 检查）。

mod common;
mod preview;

pub(crate) const TABLES: &[&[(&str, &str)]] = &[common::ENTRIES, preview::ENTRIES];
