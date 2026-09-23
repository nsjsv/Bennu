//! 受限子进程输出读取工具的共享 crate：并发收集子进程 stdout/stderr 并
//! 限制内存保留量，drain 到 EOF 避免 pipe 背压死锁。实现自 app-ui 的
//! commands/bounded_child_output 下沉（app-ui 侧 re-export 维持既有调用
//! 路径），预览命令层（bennu-preview）与 journalctl/systemctl 查询共同
//! 消费；只返回 transport 事实，领域错误解释归调用者（spec：
//! .trellis/spec/app-ui/bounded-child-output.md）。

use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Debug)]
pub struct BoundedChildOutput {
    pub bytes: Vec<u8>,
    pub exceeded_limit: bool,
}

pub async fn read_bounded_child_output(
    mut reader: impl AsyncRead + Unpin,
    maximum_bytes: usize,
) -> std::io::Result<BoundedChildOutput> {
    let mut retained_bytes = Vec::with_capacity(maximum_bytes.min(8_192));
    let mut exceeded_limit = false;
    let mut buffer = [0_u8; 8_192];

    loop {
        let bytes_read = reader.read(&mut buffer).await?;
        if bytes_read == 0 {
            break;
        }

        let available_capacity = maximum_bytes.saturating_sub(retained_bytes.len());
        let bytes_to_retain = available_capacity.min(bytes_read);
        retained_bytes.extend_from_slice(&buffer[..bytes_to_retain]);
        exceeded_limit |= bytes_to_retain < bytes_read;
    }

    Ok(BoundedChildOutput {
        bytes: retained_bytes,
        exceeded_limit,
    })
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};

    use super::read_bounded_child_output;

    struct FailingReader;

    impl AsyncRead for FailingReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            _buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::other("fixture read failure")))
        }
    }

    #[tokio::test]
    async fn retains_output_at_or_below_the_limit() {
        for (input, maximum_bytes) in [(b"abc".as_slice(), 4), (b"abcd".as_slice(), 4)] {
            let output = read_bounded_child_output(input, maximum_bytes)
                .await
                .expect("bounded output");

            assert_eq!(output.bytes, input);
            assert!(!output.exceeded_limit);
        }
    }

    #[tokio::test]
    async fn marks_excess_output_and_continues_draining() {
        let (mut sender, receiver) = tokio::io::duplex(4);
        let send = tokio::spawn(async move {
            sender.write_all(b"0123456789").await.expect("write output");
        });

        let output = read_bounded_child_output(receiver, 4)
            .await
            .expect("bounded output");
        send.await.expect("writer task");

        assert_eq!(output.bytes, b"0123");
        assert!(output.exceeded_limit);
    }

    #[tokio::test]
    async fn propagates_reader_errors() {
        let error = read_bounded_child_output(FailingReader, 4)
            .await
            .expect_err("reader error");

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(error.to_string(), "fixture read failure");
    }
}
