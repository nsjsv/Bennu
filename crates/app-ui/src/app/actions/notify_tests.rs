use std::path::PathBuf;

use super::*;
use crate::actions::notice::NoticeTemplate;

fn finished(notice: ActionNotice, stop: Option<RunStop>, failures: &[&str]) -> FinishedRun {
    FinishedRun {
        plan: super::super::run::RunPlan {
            name: "提取音频".to_owned(),
            steps: Vec::new(),
            notice,
            then: None,
        },
        results: vec![
            PathBuf::from("/music/音频/a.m4a"),
            PathBuf::from("/music/音频/b.m4a"),
        ],
        failures: failures
            .iter()
            .map(|failure| (*failure).to_owned())
            .collect(),
        stop,
    }
}

fn custom() -> ActionNotice {
    ActionNotice::Custom {
        title: NoticeTemplate::parse("音频提取好了").unwrap(),
        body: Some(NoticeTemplate::parse("{count} 个文件 → {dir}").unwrap()),
    }
}

#[test]
fn standard_notices_follow_the_other_notifications() {
    let run = finished(ActionNotice::Standard, None, &[]);
    assert_eq!(action_notification(&run, true), None);
    let shown = action_notification(&run, false).unwrap();
    assert_eq!(shown.summary, tr!("%1 completed", "提取音频"));
    assert_eq!(shown.body, tr!("%1 and %2 more", "a.m4a", 1));

    let with_failures = finished(ActionNotice::Standard, None, &["c.mp4: broken"]);
    let shown = action_notification(&with_failures, false).unwrap();
    assert!(shown.body.ends_with("c.mp4: broken"), "{}", shown.body);
}

#[test]
fn custom_notices_show_even_in_front_with_their_own_text() {
    let shown = action_notification(&finished(custom(), None, &[]), true).unwrap();
    assert_eq!(shown.summary, "音频提取好了");
    assert_eq!(shown.body, "2 个文件 → /music/音频");
}

#[test]
fn failures_always_show_and_cancels_and_silence_never_do() {
    for notice in [ActionNotice::Standard, custom()] {
        let failed = finished(
            notice.clone(),
            Some(RunStop::Failed("disk full".to_owned())),
            &[],
        );
        let shown = action_notification(&failed, true).unwrap();
        assert_eq!(shown.summary, tr!("%1 failed", "提取音频"));
        assert_eq!(shown.body, "disk full");
        assert_eq!(
            action_notification(&finished(notice, Some(RunStop::Canceled), &[]), false),
            None
        );
    }
    for stop in [None, Some(RunStop::Failed("x".to_owned()))] {
        assert_eq!(
            action_notification(&finished(ActionNotice::Silent, stop, &[]), false),
            None
        );
    }
}
