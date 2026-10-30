//! 界面层读不懂的设置项：存储层能解析出值，但值本身不认识（比如新版本加的选项、
//! 删掉的快捷键动作），只能换成默认值。这些项下次保存就会被覆盖，所以要报出来，
//! 由启动修复先备份原数据再提示用户。

/// 换成了默认值的设置项，记存储层的键名。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct UnrecognizedSettings {
    keys: Vec<&'static str>,
}

impl UnrecognizedSettings {
    /// 值认得就用它；不认得就用默认值并记下键名。
    pub(super) fn value_or_default<T>(
        &mut self,
        key: &'static str,
        recognized: Option<T>,
        default: T,
    ) -> T {
        recognized.unwrap_or_else(|| {
            self.note(key);
            default
        })
    }

    /// 列表里有元素读不懂、被丢掉时记下键名。
    pub(super) fn note_dropped(&mut self, key: &'static str, stored: usize, kept: usize) {
        if kept < stored {
            self.note(key);
        }
    }

    pub(super) fn note(&mut self, key: &'static str) {
        if !self.keys.contains(&key) {
            self.keys.push(key);
        }
    }

    pub(crate) fn into_keys(self) -> Vec<&'static str> {
        self.keys
    }
}
