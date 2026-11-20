use std::path::Path;

use file_core::FileOperationRunState;
use tokio_util::sync::CancellationToken;

use super::*;

fn shell(script: &str, directory: &Path, extra: &[&str]) -> CommandInvocation {
    CommandInvocation {
        program: "sh".into(),
        arguments: ["-c", script, "sh"]
            .iter()
            .chain(extra)
            .map(Into::into)
            .collect(),
        working_directory: directory.to_path_buf(),
    }
}

async fn run(
    invocations: Vec<CommandInvocation>,
    controls: FileOperationControls,
) -> Result<FileOperationOutcome, String> {
    let (output, _messages) = tokio::sync::mpsc::channel(64);
    run_queued_commands(invocations, controls, 7, &output).await
}

fn running() -> FileOperationControls {
    FileOperationControls::running(CancellationToken::new())
}

async fn wait_for_file(path: &Path) -> String {
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(path) {
            if text.ends_with('\n') {
                return text.trim().to_owned();
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{} never appeared", path.display());
}

fn process_is_gone(pid: libc::pid_t) -> bool {
    // 杀掉以后它可能还是没人收尸的僵尸；看 /proc 里的状态。
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => process_state(&stat) == 'Z',
    }
}

fn process_state(stat: &str) -> char {
    stat.rsplit(')')
        .next()
        .unwrap()
        .trim()
        .chars()
        .next()
        .unwrap()
}

#[tokio::test]
async fn commands_run_one_after_another_in_the_given_folder() {
    let directory = tempfile::tempdir().unwrap();
    let outcome = run(
        vec![
            shell("pwd > first", directory.path(), &[]),
            shell("cat first > second", directory.path(), &[]),
        ],
        running(),
    )
    .await;

    assert_eq!(outcome, Ok(FileOperationOutcome::NoHistory));
    assert_eq!(
        std::fs::read_to_string(directory.path().join("second"))
            .unwrap()
            .trim(),
        directory.path().canonicalize().unwrap().to_str().unwrap()
    );
}

/// 参数原样交给程序，不经过 shell：`$(…)`、`;`、引号都只是文字。
#[tokio::test]
async fn arguments_reach_the_program_byte_for_byte() {
    let directory = tempfile::tempdir().unwrap();
    let tricky = "a b; $(touch pwned) 'q' \"d\" `x`";
    let outcome = run(
        vec![shell(
            "printf '%s' \"$1\" > out",
            directory.path(),
            &[tricky],
        )],
        running(),
    )
    .await;

    assert_eq!(outcome, Ok(FileOperationOutcome::NoHistory));
    assert_eq!(
        std::fs::read_to_string(directory.path().join("out")).unwrap(),
        tricky
    );
    assert!(!directory.path().join("pwned").exists());
}

#[tokio::test]
async fn a_failing_command_stops_the_rest_and_says_why() {
    let directory = tempfile::tempdir().unwrap();
    let error = run(
        vec![
            shell(
                "echo first line >&2; echo the real reason >&2; exit 3",
                directory.path(),
                &[],
            ),
            shell("touch never", directory.path(), &[]),
        ],
        running(),
    )
    .await
    .unwrap_err();

    assert!(error.contains("sh") && error.contains('3'), "{error}");
    assert!(error.contains("the real reason"), "{error}");
    assert!(!directory.path().join("never").exists());

    let killed = run(vec![shell("kill -9 $$", directory.path(), &[])], running())
        .await
        .unwrap_err();
    assert!(killed.contains('9'), "{killed}");

    let missing = run(
        vec![CommandInvocation {
            program: "bennu-no-such-program".into(),
            arguments: Vec::new(),
            working_directory: directory.path().to_path_buf(),
        }],
        running(),
    )
    .await
    .unwrap_err();
    assert!(missing.contains("bennu-no-such-program"), "{missing}");
}

/// 取消时整组都停：命令放到后台的子进程也不留下。
#[tokio::test]
async fn canceling_stops_the_whole_process_group() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("pid");
    let cancel = CancellationToken::new();
    let controls = FileOperationControls::running(cancel.clone());
    let task = tokio::spawn(run(
        vec![shell(
            "sleep 30 & echo $! > pid; wait",
            directory.path(),
            &[],
        )],
        controls,
    ));

    let background: libc::pid_t = wait_for_file(&pid_file).await.parse().unwrap();
    cancel.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();

    assert!(outcome.is_err());
    for _ in 0..100 {
        if process_is_gone(background) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the background child {background} survived the cancel");
}

#[tokio::test]
async fn pausing_stops_the_command_and_resuming_lets_it_go_on() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("pid");
    let cancel = CancellationToken::new();
    let (run_state, receiver) = tokio::sync::watch::channel(FileOperationRunState::Running);
    let controls = FileOperationControls::new(cancel.clone(), receiver);
    let task = tokio::spawn(run(
        vec![shell("echo $$ > pid; sleep 30", directory.path(), &[])],
        controls,
    ));
    let shell_pid: libc::pid_t = wait_for_file(&pid_file).await.parse().unwrap();
    let state = || {
        std::fs::read_to_string(format!("/proc/{shell_pid}/stat"))
            .map(|stat| process_state(&stat))
            .unwrap_or('?')
    };

    run_state.send(FileOperationRunState::Paused).unwrap();
    let mut stopped = false;
    for _ in 0..100 {
        if state() == 'T' {
            stopped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(stopped, "paused command is still running: {}", state());

    run_state.send(FileOperationRunState::Running).unwrap();
    let mut resumed = false;
    for _ in 0..100 {
        if state() != 'T' {
            resumed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(resumed);

    cancel.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
    assert!(outcome.is_err());
}
