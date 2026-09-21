use file_operation_store::{
    TaskQueueStore, LAUNCH_WINDOW_POLICY_MERGE_INTO_EXISTING, LAUNCH_WINDOW_POLICY_OPEN_NEW_WINDOW,
};

/// 二次启动进程的行为：把打开请求转发给主实例合并，或自己独立运行成新窗口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaunchWindowPolicy {
    MergeIntoExisting,
    OpenNewWindow,
}

pub(crate) const DEFAULT_LAUNCH_WINDOW_POLICY: LaunchWindowPolicy =
    LaunchWindowPolicy::OpenNewWindow;

impl LaunchWindowPolicy {
    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        match value {
            LAUNCH_WINDOW_POLICY_MERGE_INTO_EXISTING => Some(Self::MergeIntoExisting),
            LAUNCH_WINDOW_POLICY_OPEN_NEW_WINDOW => Some(Self::OpenNewWindow),
            _ => None,
        }
    }

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::MergeIntoExisting => LAUNCH_WINDOW_POLICY_MERGE_INTO_EXISTING,
            Self::OpenNewWindow => LAUNCH_WINDOW_POLICY_OPEN_NEW_WINDOW,
        }
    }
}

/// 在 claim 主实例 D-Bus 名之前读取存储的策略。这是新进程决定"转发还是独立运行"的唯一时机，
/// 因此独立于完整的用户配置加载路径，任何读取失败都回退到默认策略。
pub(crate) fn stored_launch_window_policy() -> LaunchWindowPolicy {
    let state_database_path = super::default_state_database_path();
    let Ok(store) = TaskQueueStore::new(&state_database_path) else {
        return DEFAULT_LAUNCH_WINDOW_POLICY;
    };
    store
        .read_user_preferences()
        .ok()
        .flatten()
        .and_then(|stored| LaunchWindowPolicy::from_config_value(&stored.launch_window_policy))
        .unwrap_or(DEFAULT_LAUNCH_WINDOW_POLICY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_values_round_trip() {
        for policy in [
            LaunchWindowPolicy::MergeIntoExisting,
            LaunchWindowPolicy::OpenNewWindow,
        ] {
            assert_eq!(
                LaunchWindowPolicy::from_config_value(policy.config_value()),
                Some(policy)
            );
        }
    }

    #[test]
    fn unknown_config_value_falls_back_through_none() {
        assert_eq!(LaunchWindowPolicy::from_config_value("nonsense"), None);
    }
}
