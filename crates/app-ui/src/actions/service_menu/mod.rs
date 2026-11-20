//! Dolphin 服务菜单：数据目录下 `kio/servicemenus/*.desktop`（和旧的 `kservices5/`）里的动作。
//! 规则同 KIO 的 `KFileItemActions`：在哪找、信不信、对哪些项显示、放在菜单哪里、怎么执行。
//! 这里是纯模型，读文件时校验一次，之后运行时信任 `ServiceMenuAction`。

pub(crate) mod exec;
mod menu_file;
mod sources;

use std::path::PathBuf;
#[cfg(test)]
use std::sync::Arc;

pub(crate) use sources::{load_service_menus, MENU_FOLDERS};

use super::definition::{ActionId, ServiceMenuKey};
use super::type_filter::{TargetTypes, TypeFilter};
use exec::ExecTemplate;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServiceMenuAction {
    pub(crate) key: ServiceMenuKey,
    /// 本地化后的 `Name`；没写时是动作键。
    pub(crate) name: String,
    /// 只给 `%i` 用：菜单里统一用动作图标。
    pub(crate) icon: String,
    pub(crate) file: PathBuf,
    pub(crate) priority: Priority,
    /// 本地化后的 `X-KDE-Submenu`。
    pub(crate) submenu: Option<String>,
    pub(crate) types: TypeFilter,
    /// `ExcludeServiceTypes`；没写时是空列表，什么都不排除。
    pub(crate) excluded: TypeFilter,
    pub(crate) count: ItemCount,
    pub(crate) command: ExecTemplate,
}

/// `X-KDE-Priority`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Priority {
    Important,
    Normal,
    /// 直接放在主菜单里，不收进"动作"子菜单。
    TopLevel,
}

/// `X-KDE-RequiredNumberOfUrls`、`X-KDE-MinNumberOfUrls`、`X-KDE-MaxNumberOfUrls`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ItemCount {
    pub(crate) exactly: Vec<usize>,
    pub(crate) at_least: Option<usize>,
    pub(crate) at_most: Option<usize>,
}

impl ItemCount {
    fn allows(&self, count: usize) -> bool {
        (self.exactly.is_empty() || self.exactly.contains(&count))
            && self.at_least.is_none_or(|least| count >= least)
            && self.at_most.is_none_or(|most| count <= most)
    }
}

impl ServiceMenuAction {
    /// 同 KIO：数量符合，每一项都是要的类型，没有一项是排除的类型。
    pub(crate) fn applies_to(&self, target: &TargetTypes, count: usize) -> bool {
        self.count.allows(count)
            && self.types.matches_all(target)
            && !self.excluded.matches_any(target)
    }

    pub(crate) fn id(&self) -> ActionId {
        ActionId::ServiceMenu(self.key.clone())
    }

    /// 测试用：对什么都适用、放在普通位置的一项，文件是 `test.desktop`。
    #[cfg(test)]
    pub(crate) fn for_tests(action: &str, name: &str, exec: &str) -> Self {
        Self {
            key: ServiceMenuKey {
                file: Arc::from("test.desktop"),
                action: Arc::from(action),
            },
            name: name.to_owned(),
            icon: String::new(),
            file: PathBuf::from("/menus/test.desktop"),
            priority: Priority::Normal,
            submenu: None,
            types: TypeFilter::Anything,
            excluded: TypeFilter::Patterns(Vec::new()),
            count: ItemCount::default(),
            command: ExecTemplate::parse(exec).expect("a valid Exec line"),
        }
    }

    /// 同一组里的排序：按动作键（同 KIO），再按文件名，结果每次都一样。
    pub(crate) fn order_key(&self) -> (&str, &str) {
        (&self.key.action, &self.key.file)
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
