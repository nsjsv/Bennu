//! 成员密码弹窗状态机的单元测试:打开/提交重试/取消/密码错再弹的
//! 全部转移都在状态层面断言,不起真实提取(加密包集成路径另有验收)。

use std::path::PathBuf;

use file_core::ArchivePassword;

use super::archive_member_password::{
    ArchiveMemberOpenOutcome, ArchiveMemberPasswordAction, ArchiveMemberPasswordMessage,
};
use super::archive_password::ArchivePasswordDraft;
use super::FileBrowser;
use crate::model::Message;
use crate::operation_queue::QueuedFileOperation;

fn extract_action() -> ArchiveMemberPasswordAction {
    ArchiveMemberPasswordAction::Extract {
        sources: vec![PathBuf::from("/tmp/docs.rar/photo.png")],
        destination: PathBuf::from("/tmp/landing"),
    }
}

fn open_action() -> ArchiveMemberPasswordAction {
    ArchiveMemberPasswordAction::Open {
        path: PathBuf::from("/tmp/docs.rar/photo.png"),
    }
}

fn request_password(browser: &mut FileBrowser, action: ArchiveMemberPasswordAction, invalid: bool) {
    drop(browser.update(Message::ArchiveMemberPasswordRequested {
        action,
        invalid_retry: invalid,
    }));
}

fn type_password(browser: &mut FileBrowser, password: &str) {
    drop(browser.update(Message::ArchiveMemberPassword(
        ArchiveMemberPasswordMessage::PasswordChanged(ArchivePasswordDraft::new(
            password.to_owned(),
        )),
    )));
}

fn submit(browser: &mut FileBrowser) {
    drop(browser.update(Message::ArchiveMemberPassword(
        ArchiveMemberPasswordMessage::Submitted,
    )));
}

fn open_finished(browser: &mut FileBrowser, outcome: ArchiveMemberOpenOutcome) {
    drop(browser.update(Message::ArchiveMemberPassword(
        ArchiveMemberPasswordMessage::OpenFinished {
            path: PathBuf::from("/tmp/docs.rar/photo.png"),
            outcome,
        },
    )));
}

#[test]
fn requested_dialog_waits_for_password_without_error() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    request_password(&mut browser, extract_action(), false);

    let state = browser.archive_member_password.as_ref().unwrap();
    assert_eq!(state.action(), &extract_action());
    assert!(state.can_submit_password());
    assert_eq!(state.validation_error(), None);
}

#[test]
fn invalid_retry_request_shows_incorrect_password_hint() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    request_password(&mut browser, extract_action(), true);

    let state = browser.archive_member_password.as_ref().unwrap();
    assert_eq!(
        state.validation_error(),
        Some("Incorrect password. Try again.")
    );
}

#[test]
fn submit_without_password_keeps_dialog_with_validation_error() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    request_password(&mut browser, extract_action(), false);
    submit(&mut browser);

    let state = browser.archive_member_password.as_ref().unwrap();
    assert_eq!(
        state.validation_error(),
        Some("Enter the archive password.")
    );
}

#[test]
fn extract_submit_enqueues_members_with_password_and_closes_dialog() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    request_password(&mut browser, extract_action(), false);
    type_password(&mut browser, "secret");
    submit(&mut browser);

    // 提交即带密码重新入队并关闭弹窗;密码错误由队列失败通道再弹。
    assert!(browser.archive_member_password.is_none());
    assert_eq!(browser.operation_queue.tasks().len(), 1);
    let queued = &browser.operation_queue.tasks()[0].operation;
    let queued_password = match queued {
        QueuedFileOperation::ExtractArchiveMembers {
            sources,
            destination,
            password,
        } => {
            assert_eq!(sources, &vec![PathBuf::from("/tmp/docs.rar/photo.png")]);
            assert_eq!(destination, &PathBuf::from("/tmp/landing"));
            password.clone()
        }
        other => panic!("unexpected queued operation: {other:?}"),
    };
    // ArchivePassword::new 对空串返回 None,非空即 Some(密码)。
    assert_eq!(queued_password, ArchivePassword::new("secret".to_owned()));
}

#[test]
fn open_submit_starts_retry_and_invalid_result_reprompts() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    request_password(&mut browser, open_action(), false);
    type_password(&mut browser, "guess");
    submit(&mut browser);

    // Open 提交不关弹窗:重试在途期间不能重复提交(按钮禁用)。
    let state = browser.archive_member_password.as_ref().unwrap();
    assert!(!state.can_submit_password());

    open_finished(&mut browser, ArchiveMemberOpenOutcome::InvalidPassword);

    // 密码错回到等待输入态并提示重试,与整包解压弹窗交互一致。
    let state = browser.archive_member_password.as_ref().unwrap();
    assert!(state.can_submit_password());
    assert_eq!(
        state.validation_error(),
        Some("Incorrect password. Try again.")
    );
}

#[test]
fn open_retry_success_closes_dialog() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    request_password(&mut browser, open_action(), false);
    type_password(&mut browser, "secret");
    submit(&mut browser);
    open_finished(&mut browser, ArchiveMemberOpenOutcome::Opened);

    assert!(browser.archive_member_password.is_none());
}

#[test]
fn cancelled_dialog_ignores_late_invalid_password_result() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    request_password(&mut browser, open_action(), false);
    type_password(&mut browser, "guess");
    submit(&mut browser);
    // 用户在重试在途时取消(与 DismissFloating 清理同效)。
    browser.archive_member_password = None;

    open_finished(&mut browser, ArchiveMemberOpenOutcome::InvalidPassword);

    // 尊重取消,不重新弹窗(与整包解压丢弃迟到检查结果一致)。
    assert!(browser.archive_member_password.is_none());
}

#[test]
fn cancelled_dialog_ignores_late_failed_result() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    request_password(&mut browser, open_action(), false);
    type_password(&mut browser, "guess");
    submit(&mut browser);
    browser.archive_member_password = None;

    // 迟到的失败结果同样丢弃:不反向弹全局错误,与整包解压语义一致。
    open_finished(
        &mut browser,
        ArchiveMemberOpenOutcome::Failed("member vanished".to_owned()),
    );

    assert!(browser.archive_member_password.is_none());
    assert!(browser.global_error_notification.is_none());
}

#[test]
fn switched_dialog_ignores_open_result_for_other_action() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    request_password(&mut browser, open_action(), false);
    type_password(&mut browser, "guess");
    submit(&mut browser);

    // 弹窗被队列失败通道切换为提取动作(同模态互斥清理):原 Open
    // 重试的终态不属于当前弹窗,必须整批丢弃,不能误关新弹窗。
    request_password(&mut browser, extract_action(), true);
    let typed = ArchivePasswordDraft::new("typed".to_owned());
    drop(browser.update(Message::ArchiveMemberPassword(
        ArchiveMemberPasswordMessage::PasswordChanged(typed),
    )));

    open_finished(&mut browser, ArchiveMemberOpenOutcome::InvalidPassword);

    let state = browser.archive_member_password.as_ref().unwrap();
    assert_eq!(state.action(), &extract_action());
    assert!(state.can_submit_password());
    // 输入中的新密码不被迟到结果清掉。
    assert_eq!(state.password().as_str(), "typed");
}

#[test]
fn extract_dialog_reopens_fresh_after_queue_failure() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    request_password(&mut browser, extract_action(), false);
    type_password(&mut browser, "guess");
    submit(&mut browser);
    assert!(browser.archive_member_password.is_none());

    // 队列重试再次密码错:弹窗被提交时已关闭,失败通道重新开一个
    // 新弹窗并带 Incorrect password 提示。
    request_password(&mut browser, extract_action(), true);

    let state = browser.archive_member_password.as_ref().unwrap();
    assert!(state.can_submit_password());
    assert_eq!(
        state.validation_error(),
        Some("Incorrect password. Try again.")
    );
}

#[test]
fn member_password_dialog_is_cleared_with_archive_extraction_state() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    request_password(&mut browser, extract_action(), false);
    assert!(browser.archive_member_password.is_some());

    // 两个模态弹窗互斥:打开整包解压弹窗的入口清理会一并关掉成员弹窗。
    browser.clear_state_for_archive_extraction();

    assert!(browser.archive_member_password.is_none());
}
