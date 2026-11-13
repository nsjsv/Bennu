use std::path::Path;
use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::*;

#[test]
fn progress_segments_follow_extract_and_add_layouts() {
    assert_eq!(
        parse_seven_zip_progress("  24% 2 - big/text1.txt"),
        Some(SevenZipProgress {
            percent: 24,
            completed_items: 2
        })
    );
    assert_eq!(
        parse_seven_zip_progress(" 41% 1 + big/text2.txt"),
        Some(SevenZipProgress {
            percent: 41,
            completed_items: 1
        })
    );
    assert_eq!(
        parse_seven_zip_progress("  8% + big/100%.txt"),
        Some(SevenZipProgress {
            percent: 8,
            completed_items: 0
        })
    );
    assert_eq!(parse_seven_zip_progress("  0M Scan"), None);
    assert_eq!(parse_seven_zip_progress("      "), None);
}

/// 管道里没有换行，只有 `\b` 擦除；读到一半的段要等下一块数据补齐。
#[test]
fn progress_stream_splits_on_backspaces_across_reads() {
    let mut stream = SevenZipProgressStream::default();
    let mut seen = Vec::new();

    stream.push(
        b"  0M Scan\x08\x08\x08\x08  0%\x08\x08\x08\x08 2",
        &mut |progress| seen.push(progress),
    );
    stream.push(
        b"4% 2 - a.bin\x08\x08\x08    \x08\x08\x08",
        &mut |progress| seen.push(progress),
    );

    assert_eq!(
        seen,
        vec![
            SevenZipProgress {
                percent: 0,
                completed_items: 0
            },
            SevenZipProgress {
                percent: 24,
                completed_items: 2
            },
        ]
    );
}

fn process_state(pid: &str) -> char {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    // 第三个字段是状态；进程名在括号里，可能含空格，从右括号之后数。
    stat.rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .next()
        .unwrap()
        .chars()
        .next()
        .unwrap()
}

async fn wait_for_pid(pid_file: &Path) -> String {
    for _ in 0..200 {
        if let Ok(pid) = std::fs::read_to_string(pid_file) {
            if !pid.trim().is_empty() {
                return pid.trim().to_owned();
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("child did not write its pid");
}

async fn wait_for_state(pid: &str, expected: char) {
    for _ in 0..200 {
        if process_state(pid) == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("process {pid} never reached state {expected}");
}

/// 暂停要真的冻结子进程（状态 T），继续后恢复；取消时即使正停着也会被杀掉。
#[tokio::test]
async fn pause_stops_the_child_and_cancel_kills_it_even_while_stopped() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("pid");
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("echo $$ > \"$1\"; exec sleep 30")
        .arg("sh")
        .arg(&pid_file);
    let (run_state_sender, run_state) = watch::channel(FileOperationRunState::Running);
    let cancel = CancellationToken::new();
    let mut controls = FileOperationControls::new(cancel.clone(), run_state);

    let job = tokio::spawn(async move { run_seven_zip_job(command, &mut controls, |_| {}).await });
    let pid = wait_for_pid(&pid_file).await;

    run_state_sender
        .send(FileOperationRunState::Paused)
        .unwrap();
    wait_for_state(&pid, 'T').await;
    run_state_sender
        .send(FileOperationRunState::Running)
        .unwrap();
    wait_for_state(&pid, 'S').await;
    run_state_sender
        .send(FileOperationRunState::Paused)
        .unwrap();
    wait_for_state(&pid, 'T').await;
    cancel.cancel();

    let outcome = tokio::time::timeout(Duration::from_secs(5), job)
        .await
        .expect("a stopped child must still be killed on cancel")
        .unwrap();
    assert!(matches!(
        outcome,
        Err(SevenZipJobFailure::Interrupted(FileError::Cancelled))
    ));
}

#[tokio::test]
async fn progress_and_stderr_reach_the_caller() {
    let mut command = Command::new("sh");
    command.arg("-c").arg(
        "printf '  0%%\\b\\b\\b\\b 37%% 3 - a\\b\\b\\b\\b\\b\\b\\b\\b\\b\\b'; \
         echo 'ERROR: Wrong password?' >&2; exit 2",
    );
    let mut controls = FileOperationControls::running(CancellationToken::new());
    let mut seen = Vec::new();

    let outcome = run_seven_zip_job(command, &mut controls, |progress| seen.push(progress)).await;

    assert_eq!(
        seen.last(),
        Some(&SevenZipProgress {
            percent: 37,
            completed_items: 3
        })
    );
    let Err(SevenZipJobFailure::Exited { stderr, .. }) = outcome else {
        panic!("expected a failed exit");
    };
    assert_eq!(stderr, "ERROR: Wrong password?");
}
