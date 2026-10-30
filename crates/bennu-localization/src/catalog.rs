//! 文案表：每条是（英文句式，中文句式），英文句式就是 `tr!` 的消息 ID。按界面区域
//! 分文件；同一句英文全表只能出现一次（`message_tests` 检查）。

mod common;
mod files;
mod operations;
mod preview;
mod search;
mod settings;
mod tools;
mod transfer;

pub(crate) const TABLES: &[&[(&str, &str)]] = &[
    common::ENTRIES,
    files::ENTRIES,
    operations::ENTRIES,
    preview::ENTRIES,
    search::ENTRIES,
    settings::ENTRIES,
    tools::ENTRIES,
    transfer::ENTRIES,
];
