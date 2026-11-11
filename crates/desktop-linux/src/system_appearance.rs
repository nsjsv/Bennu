//! 系统深浅色偏好：读自桌面 portal 的 `org.freedesktop.appearance` `color-scheme`，
//! 并跟着系统设置实时变。主程序和文件选择对话框
//! 都从这里读，所以两个进程同时换色。
//!
//! 系统强调色不读：默认配色是固定的黑白灰，出名配色用自己的强调色（2026-11-10 决定）。

use futures_util::{stream, Stream, StreamExt};
use zbus::zvariant::{OwnedValue, Value};

const APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";
const COLOR_SCHEME_KEY: &str = "color-scheme";
/// portal 规范：`color-scheme` 0 = 没偏好、1 = 深色、2 = 浅色。
const PREFERS_DARK: u32 = 1;

#[zbus::proxy(
    interface = "org.freedesktop.portal.Settings",
    default_service = "org.freedesktop.portal.Desktop",
    default_path = "/org/freedesktop/portal/desktop"
)]
trait PortalSettings {
    fn read_one(&self, namespace: &str, key: &str) -> zbus::Result<OwnedValue>;

    /// 老 portal（没有 `ReadOne`）只有它，返回值多包一层 variant。
    fn read(&self, namespace: &str, key: &str) -> zbus::Result<OwnedValue>;

    #[zbus(signal)]
    fn setting_changed(&self, namespace: &str, key: &str, value: Value<'_>) -> zbus::Result<()>;
}

/// 系统是否偏好深色：第一项是当前值，之后系统每切换一次出一项。连不上 portal（没有
/// 会话总线、桌面没实现 Settings）时只出一项 `false`（浅色）就结束，不重连。
pub fn portal_prefers_dark_changes() -> impl Stream<Item = bool> + Send + 'static {
    stream::unfold(WatchState::Connecting, |state| async move {
        match state {
            WatchState::Connecting => match connect().await {
                None => Some((false, WatchState::Finished)),
                Some((proxy, changes)) => {
                    let prefers_dark = read_color_scheme(&proxy)
                        .await
                        .is_some_and(|value| value_prefers_dark(&value));
                    Some((
                        prefers_dark,
                        WatchState::Watching {
                            _proxy: proxy,
                            changes: Box::new(changes),
                            prefers_dark,
                        },
                    ))
                }
            },
            WatchState::Watching {
                _proxy,
                mut changes,
                prefers_dark,
            } => loop {
                let signal = changes.next().await?;
                let Some(changed) = changed_preference(&signal) else {
                    continue;
                };
                if changed != prefers_dark {
                    let next = WatchState::Watching {
                        _proxy,
                        changes,
                        prefers_dark: changed,
                    };
                    return Some((changed, next));
                }
            },
            WatchState::Finished => None,
        }
    })
}

enum WatchState {
    Connecting,
    Watching {
        /// 信号流只认连接；留着代理，连接不会被提前关掉。
        _proxy: PortalSettingsProxy<'static>,
        /// 装箱：信号流比另外两种状态大几百字节。
        changes: Box<SettingChangedStream>,
        prefers_dark: bool,
    },
    Finished,
}

/// 只认外观命名空间的 `color-scheme`；别的设置、读不懂的信号：`None`。
fn changed_preference(signal: &SettingChanged) -> Option<bool> {
    let args = signal.args().ok()?;
    (args.namespace == APPEARANCE_NAMESPACE && args.key == COLOR_SCHEME_KEY)
        .then(|| value_prefers_dark(&args.value))
}

async fn connect() -> Option<(PortalSettingsProxy<'static>, SettingChangedStream)> {
    let connection = zbus::Connection::session()
        .await
        .inspect_err(|error| tracing::info!(%error, "no session bus; using the light appearance"))
        .ok()?;
    let proxy = PortalSettingsProxy::new(&connection).await.ok()?;
    // 先订阅再读，读和订阅之间的改动不会丢。
    let changes = proxy
        .receive_setting_changed()
        .await
        .inspect_err(|error| tracing::info!(%error, "settings portal is unavailable"))
        .ok()?;
    Some((proxy, changes))
}

/// 没设置、portal 不认识这个键：`None`。
async fn read_color_scheme(proxy: &PortalSettingsProxy<'_>) -> Option<OwnedValue> {
    match proxy.read_one(APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY).await {
        Ok(value) => Some(value),
        Err(zbus::Error::MethodError(name, _, _))
            if name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod" =>
        {
            proxy
                .read(APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY)
                .await
                .ok()
        }
        Err(_) => None,
    }
}

fn value_prefers_dark(value: &Value<'_>) -> bool {
    // `Read` 的返回值多包了一层 variant。
    let value = match value {
        Value::Value(inner) => inner,
        value => value,
    };
    u32::try_from(value).is_ok_and(|scheme| scheme == PREFERS_DARK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_dark_preference_means_dark() {
        assert!(value_prefers_dark(&Value::U32(1)));
        assert!(!value_prefers_dark(&Value::U32(0)));
        assert!(!value_prefers_dark(&Value::U32(2)));
        assert!(!value_prefers_dark(&Value::Str("dark".into())));
    }

    #[test]
    fn values_from_the_old_read_method_are_unwrapped_once() {
        assert!(value_prefers_dark(&Value::Value(Box::new(Value::U32(1)))));
        assert!(!value_prefers_dark(&Value::Value(Box::new(Value::U32(2)))));
    }
}
