//! A5：二维码下载服务（明文 HTTP，浏览器没有客户端证书）。

use std::time::Duration;

use super::{client, wait_for};
use crate::{QrDownloadServer, QrEvent, QrItem, QrSource};

fn token_of(url: &str) -> String {
    url.rsplit('/').next().unwrap().to_string()
}

fn port_of(url: &str) -> u16 {
    let host_port = url.trim_start_matches("http://").split('/').next().unwrap();
    host_port.rsplit(':').next().unwrap().parse().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn qr_serves_index_files_and_zip_then_finishes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("alpha.txt"), b"AAA").unwrap();
    let nested = dir.path().join("docs");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("readme.md"), b"hello zip").unwrap();

    let handle = QrDownloadServer::start_with_idle_timeout(
        vec![
            QrItem {
                display_name: "alpha.txt".into(),
                source: QrSource::File(dir.path().join("alpha.txt")),
            },
            QrItem {
                display_name: "docs".into(),
                source: QrSource::Dir(nested.clone()),
            },
        ],
        Duration::from_secs(120),
    )
    .await
    .unwrap();
    // 测试环境必须存在局域网候选地址（与此前 local_ip 路径的隐含前提一致）。
    let primary_url = handle
        .urls()
        .first()
        .expect("qr urls must contain at least one candidate address")
        .clone();
    let token = token_of(&primary_url);
    let port = port_of(&primary_url);
    let base = format!("http://127.0.0.1:{port}");
    let mut events = handle.events();

    // token 是唯一鉴权：错误 token 一律 404。
    assert_eq!(
        client()
            .get(format!("{base}/wrong-token"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    // 列表页含条目与 zip 链接；首个请求产生 Connected。
    let index = client()
        .get(format!("{base}/{token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(index.status(), 200);
    let page = index.text().await.unwrap();
    assert!(page.contains("alpha.txt"));
    assert!(page.contains("/zip"));
    wait_for(&mut events, |e| matches!(e, QrEvent::Connected)).await;

    // 单文件流：内容一致。
    let file = client()
        .get(format!("{base}/{token}/file/0"))
        .send()
        .await
        .unwrap();
    assert_eq!(file.status(), 200);
    assert_eq!(file.bytes().await.unwrap().as_ref(), b"AAA");
    // 不存在的条目 404；目录条目没有单文件下载。
    assert_eq!(
        client()
            .get(format!("{base}/{token}/file/9"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client()
            .get(format!("{base}/{token}/file/1"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    // zip：async_zip 读回校验条目与内容。
    let zip = client()
        .get(format!("{base}/{token}/zip"))
        .send()
        .await
        .unwrap();
    assert_eq!(zip.status(), 200);
    let zip_bytes = zip.bytes().await.unwrap().to_vec();
    let mut reader =
        async_zip::tokio::read::seek::ZipFileReader::with_tokio(std::io::Cursor::new(zip_bytes))
            .await
            .unwrap();
    // zip 只写文件条目（目录由路径隐含）：alpha.txt + docs/readme.md。
    assert_eq!(reader.file().entries().len(), 2);
    let mut contents = Vec::new();
    for index in 0..reader.file().entries().len() {
        // async_zip 的 entry reader 实现 futures AsyncRead，需 compat 包装成 tokio 侧。
        use tokio_util::compat::FuturesAsyncReadCompatExt;
        let mut entry_reader = reader.reader_with_entry(index).await.unwrap().compat();
        let mut buffer = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut entry_reader, &mut buffer)
            .await
            .unwrap();
        contents.push((
            reader.file().entries()[index]
                .filename()
                .clone()
                .into_string()
                .unwrap(),
            buffer,
        ));
    }
    assert!(contents.contains(&("alpha.txt".to_string(), b"AAA".to_vec())));
    assert!(contents.contains(&("docs/readme.md".to_string(), b"hello zip".to_vec())));

    // 全部条目下载完成 → 当场 Finished，不等定时检查。
    tokio::time::timeout(
        Duration::from_millis(200),
        wait_for(&mut events, |e| matches!(e, QrEvent::Finished)),
    )
    .await
    .expect("finished right after the last download");
    // 传完自动关：端口不再监听（A4 验收口径）。
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn qr_times_out_when_idle_and_shuts_down() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("idle.txt"), b"z").unwrap();
    let handle = QrDownloadServer::start_with_idle_timeout(
        vec![QrItem {
            display_name: "idle.txt".into(),
            source: QrSource::File(dir.path().join("idle.txt")),
        }],
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    let port = port_of(handle.urls().first().expect("qr urls non-empty"));
    let mut events = handle.events();
    wait_for(&mut events, |e| matches!(e, QrEvent::TimedOut)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn qr_handle_drop_stops_server() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("x.txt"), b"x").unwrap();
    let port = {
        let handle = QrDownloadServer::start_with_idle_timeout(
            vec![QrItem {
                display_name: "x.txt".into(),
                source: QrSource::File(dir.path().join("x.txt")),
            }],
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        port_of(handle.urls().first().expect("qr urls non-empty"))
    };
    // 句柄 drop（R4：关窗即关服务）→ 监听停止。
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .is_err());
}
