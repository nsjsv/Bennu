use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use file_search::{
    status_via_socket, version_via_socket, IndexedQueryAvailability, SearchDatabase,
    SearchServicePhase,
};
use tempfile::{tempdir, TempDir};
use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;
use tokio::process::Command;

#[tokio::test]
async fn sigterm_reuses_protocol_shutdown_completion_boundary() {
    let fixture = DaemonProcessFixture::new();
    let mut daemon_process = fixture.daemon_command().spawn().unwrap();

    wait_for_endpoint(&mut daemon_process, &fixture.socket_path).await;
    let mut idle_client = UnixStream::connect(&fixture.socket_path).await.unwrap();
    let daemon_process_id = daemon_process.id().unwrap();
    let signal_status = unsafe { libc::kill(daemon_process_id as i32, libc::SIGTERM) };
    assert_eq!(signal_status, 0, "failed to send SIGTERM to bennu-searchd");

    let exit_status = tokio::time::timeout(Duration::from_secs(30), daemon_process.wait())
        .await
        .expect("bennu-searchd did not finish graceful shutdown")
        .unwrap();
    assert!(
        exit_status.success(),
        "bennu-searchd did not exit successfully after SIGTERM: {exit_status}"
    );

    let mut trailing_byte = [0_u8; 1];
    let bytes_read =
        tokio::time::timeout(Duration::from_secs(1), idle_client.read(&mut trailing_byte))
            .await
            .expect("idle client did not observe shutdown EOF")
            .unwrap();
    assert_eq!(bytes_read, 0, "idle client received data instead of EOF");
    assert!(
        !fixture.socket_path.exists(),
        "daemon socket survived shutdown"
    );
}

#[tokio::test]
async fn sigterm_during_locked_database_maintenance_exits_without_waiting_for_sqlite() {
    let fixture = DaemonProcessFixture::new();
    let database_path = fixture.database_path();
    std::fs::create_dir_all(database_path.parent().unwrap()).unwrap();
    drop(SearchDatabase::open(&database_path).unwrap());

    let locking_connection = rusqlite::Connection::open(&database_path).unwrap();
    locking_connection
        .execute_batch("BEGIN; SELECT COUNT(*) FROM sqlite_master;")
        .unwrap();
    let wal_writer = rusqlite::Connection::open(&database_path).unwrap();
    wal_writer
        .execute_batch(
            "CREATE TABLE shutdown_lock_fixture (value INTEGER NOT NULL);
             INSERT INTO shutdown_lock_fixture VALUES (1);",
        )
        .unwrap();
    drop(wal_writer);
    assert!(
        PathBuf::from(format!("{}-wal", database_path.display())).exists(),
        "fixture did not create a WAL file"
    );

    let mut daemon_process = fixture.daemon_command().spawn().unwrap();
    wait_for_endpoint(&mut daemon_process, &fixture.socket_path).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let status = status_via_socket(&fixture.socket_path).await.unwrap();
    assert_eq!(
        status.phase,
        SearchServicePhase::Ready,
        "database maintenance prevented the daemon from becoming Ready"
    );
    assert_eq!(
        status.query_availability,
        IndexedQueryAvailability::Available,
        "database maintenance prevented indexed queries"
    );

    let daemon_process_id = daemon_process.id().unwrap();
    let signal_status = unsafe { libc::kill(daemon_process_id as i32, libc::SIGTERM) };
    assert_eq!(signal_status, 0, "failed to send SIGTERM to bennu-searchd");
    let exit_status = tokio::time::timeout(Duration::from_secs(2), daemon_process.wait())
        .await
        .expect("bennu-searchd waited for locked SQLite maintenance")
        .unwrap();

    assert!(
        exit_status.success(),
        "bennu-searchd did not exit successfully after SIGTERM: {exit_status}"
    );
    assert!(
        !fixture.socket_path.exists(),
        "daemon socket survived shutdown during maintenance"
    );
    locking_connection.execute_batch("ROLLBACK;").unwrap();
}

#[tokio::test]
async fn shutdown_existing_retires_running_file_searchd_and_removes_socket() {
    let fixture = DaemonProcessFixture::new();
    let mut daemon_process = fixture.daemon_command().spawn().unwrap();
    wait_for_endpoint(&mut daemon_process, &fixture.socket_path).await;

    let mut shutdown_command = fixture.daemon_command();
    let shutdown_exit_status = shutdown_command
        .arg("--shutdown-existing")
        .status()
        .await
        .unwrap();
    assert!(
        shutdown_exit_status.success(),
        "--shutdown-existing did not exit successfully: {shutdown_exit_status}"
    );

    let daemon_exit_status = tokio::time::timeout(Duration::from_secs(30), daemon_process.wait())
        .await
        .expect("legacy bennu-searchd did not finish protocol shutdown")
        .unwrap();
    assert!(
        daemon_exit_status.success(),
        "legacy bennu-searchd did not exit successfully: {daemon_exit_status}"
    );
    assert!(
        !fixture.socket_path.exists(),
        "daemon socket survived --shutdown-existing"
    );
}

#[tokio::test]
async fn shutdown_existing_refuses_unknown_socket_owner_without_removing_socket() {
    let fixture = DaemonProcessFixture::new();
    let unknown_owner = std::os::unix::net::UnixListener::bind(&fixture.socket_path).unwrap();

    let mut shutdown_command = fixture.daemon_command();
    let shutdown_exit_status = shutdown_command
        .arg("--shutdown-existing")
        .status()
        .await
        .unwrap();

    assert!(
        !shutdown_exit_status.success(),
        "--shutdown-existing accepted an unknown socket owner"
    );
    assert!(
        fixture.socket_path.exists(),
        "unknown owner socket was removed"
    );
    drop(unknown_owner);
    std::fs::remove_file(&fixture.socket_path).unwrap();
}

struct DaemonProcessFixture {
    _temporary_directory: TempDir,
    home_directory: PathBuf,
    runtime_directory: PathBuf,
    data_directory: PathBuf,
    cache_directory: PathBuf,
    config_directory: PathBuf,
    socket_path: PathBuf,
}

impl DaemonProcessFixture {
    fn new() -> Self {
        let temporary_directory = tempdir().unwrap();
        let home_directory = temporary_directory.path().join("home");
        let runtime_directory = temporary_directory.path().join("runtime");
        let data_directory = temporary_directory.path().join("data");
        let cache_directory = temporary_directory.path().join("cache");
        let config_directory = temporary_directory.path().join("config");
        for directory in [
            &home_directory,
            &runtime_directory,
            &data_directory,
            &cache_directory,
            &config_directory,
        ] {
            std::fs::create_dir_all(directory).unwrap();
        }

        let socket_path = runtime_directory.join("bennu-search.sock");
        Self {
            _temporary_directory: temporary_directory,
            home_directory,
            runtime_directory,
            data_directory,
            cache_directory,
            config_directory,
            socket_path,
        }
    }

    fn database_path(&self) -> PathBuf {
        self.data_directory.join("bennu/search.sqlite")
    }

    fn daemon_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bennu-searchd"));
        command
            .env("HOME", &self.home_directory)
            .env("XDG_RUNTIME_DIR", &self.runtime_directory)
            .env("XDG_DATA_HOME", &self.data_directory)
            .env("XDG_CACHE_HOME", &self.cache_directory)
            .env("XDG_CONFIG_HOME", &self.config_directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        command
    }
}

async fn wait_for_endpoint(daemon_process: &mut tokio::process::Child, socket_path: &Path) {
    let startup_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if version_via_socket(socket_path).await.is_ok() {
            return;
        }
        if let Some(exit_status) = daemon_process.try_wait().unwrap() {
            panic!("bennu-searchd exited before endpoint readiness: {exit_status}");
        }
        assert!(
            Instant::now() < startup_deadline,
            "bennu-searchd endpoint did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
