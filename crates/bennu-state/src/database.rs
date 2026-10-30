//! 每进程对同一个 `state.sqlite` 只持有一份连接、只建一次表。
//!
//! 旧实现每次调用都 `Connection::open` 再设三条 PRAGMA，启动时还会为每个读取方重复建表。
//! 现在按路径登记句柄：同一进程里谁先打开谁负责初始化，之后拿到的都是同一份。登记表只存
//! 弱引用：测试每个用例一个临时库，强引用会把文件描述符留到进程结束；真实进程在入口处
//! 持有一份句柄，启动期零散的读取就不会反复重建。
//!
//! 一写一读两条连接：WAL 下读者不被写者阻塞。界面线程会读任务表（队列监督），不能被后台
//! 大批量清单落盘的长事务卡住，所以读取不与写入共用一把锁。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::StoreResult;

static OPEN_DATABASES: Mutex<BTreeMap<PathBuf, Weak<OpenDatabase>>> = Mutex::new(BTreeMap::new());

#[derive(Debug)]
struct OpenDatabase {
    path: PathBuf,
    writer: Mutex<Connection>,
    reader: Mutex<Connection>,
}

#[derive(Debug, Clone)]
pub struct StateDatabase {
    open: Arc<OpenDatabase>,
}

impl StateDatabase {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let path = registry_key(path.as_ref())?;
        // 持登记表锁完成初始化：同一路径的并发 open 只会有一个负责建表。
        let mut open_databases = OPEN_DATABASES
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(open) = open_databases.get(&path).and_then(Weak::upgrade) {
            return Ok(Self { open });
        }
        let open = Arc::new(OpenDatabase::create(path.clone())?);
        open_databases.retain(|_, released| released.strong_count() > 0);
        open_databases.insert(path, Arc::downgrade(&open));
        Ok(Self { open })
    }

    pub fn path(&self) -> &Path {
        &self.open.path
    }

    pub(crate) fn write(&self) -> MutexGuard<'_, Connection> {
        lock_connection(&self.open.writer)
    }

    /// 只读连接设了 `query_only`：误把写入放到这里会立即报错，而不是悄悄绕过写锁。
    pub(crate) fn read(&self) -> MutexGuard<'_, Connection> {
        lock_connection(&self.open.reader)
    }
}

impl OpenDatabase {
    fn create(path: PathBuf) -> StoreResult<Self> {
        let writer = open_connection(&path)?;
        writer.pragma_update(None, "journal_mode", "WAL")?;
        for schema in [
            crate::task_queue::SCHEMA_SQL,
            crate::recoverable_transfer::SCHEMA_SQL,
            crate::user_settings::SCHEMA_SQL,
            crate::browser_session::SCHEMA_SQL,
        ] {
            writer.execute_batch(schema)?;
        }
        let reader = open_connection(&path)?;
        reader.pragma_update(None, "query_only", "ON")?;
        Ok(Self {
            path,
            writer: Mutex::new(writer),
            reader: Mutex::new(reader),
        })
    }
}

/// 各表 `updated_at_ms` 列共用的时钟。
pub(crate) fn current_time_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

fn open_connection(path: &Path) -> StoreResult<Connection> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(connection)
}

/// 某个线程在持锁时 panic 不会让连接失效：SQLite 事务随栈展开回滚，
/// 所以中毒的锁照常使用，不把一次 panic 扩散成此后所有存储调用失败。
fn lock_connection(connection: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    connection.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 同一个文件的不同写法（相对路径、符号链接、`..`）归到同一个键，
/// 否则会各自开一份连接，回到"每个调用方自己管连接"的老问题。
fn registry_key(path: &Path) -> StoreResult<PathBuf> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(directory)?;
    Ok(match path.file_name() {
        Some(file_name) => directory.canonicalize()?.join(file_name),
        None => path.to_path_buf(),
    })
}

#[cfg(test)]
#[path = "database_tests.rs"]
mod tests;
