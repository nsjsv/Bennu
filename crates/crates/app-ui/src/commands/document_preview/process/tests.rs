use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::*;

#[tokio::test]
async fn pipe_cleanup_joins_both_bounded_readers_before_returning() {
    let stdout_joined = Arc::new(AtomicBool::new(false));
    let stderr_joined = Arc::new(AtomicBool::new(false));
    let stdout_flag = stdout_joined.clone();
    let stderr_flag = stderr_joined.clone();
    let mut stdout_reader = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        stdout_flag.store(true, Ordering::SeqCst);
        Ok(BoundedChildOutput {
            bytes: Vec::new(),
            exceeded_limit: false,
        })
    });
    let mut stderr_reader = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        stderr_flag.store(true, Ordering::SeqCst);
        Ok(BoundedChildOutput {
            bytes: Vec::new(),
            exceeded_limit: false,
        })
    });
    let mut stdout_result = None;
    let mut stderr_result = None;

    finish_uncompleted_pipe_task(&mut stdout_reader, &mut stdout_result, "stdout")
        .await
        .unwrap();
    finish_uncompleted_pipe_task(&mut stderr_reader, &mut stderr_result, "stderr")
        .await
        .unwrap();

    assert!(stdout_joined.load(Ordering::SeqCst));
    assert!(stderr_joined.load(Ordering::SeqCst));
    assert!(matches!(stdout_result, Some(Ok(_))));
    assert!(matches!(stderr_result, Some(Ok(_))));
}

#[tokio::test]
async fn pipe_cleanup_aborts_a_reader_after_the_fixed_deadline() {
    let mut reader: JoinHandle<io::Result<BoundedChildOutput>> = tokio::spawn(async {
        std::future::pending::<()>().await;
        Ok(BoundedChildOutput {
            bytes: Vec::new(),
            exceeded_limit: false,
        })
    });
    let mut result = None;
    let started = tokio::time::Instant::now();

    let error = finish_uncompleted_pipe_task(&mut reader, &mut result, "stdout")
        .await
        .expect_err("stuck reader");

    assert!(error.contains("reader did not stop"));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(reader.is_finished());
}
