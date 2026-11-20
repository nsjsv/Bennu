//! 把表单的改动写进动作文件的 TOML 文档。只碰变了的字段：没动的部分（注释、空行、键的顺序、
//! 引号的写法）原样不动；步骤搬家时，它上面的注释跟着走。

use file_core::mime::MimeDatabase;
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

use super::super::action_file::{spelling, EXTRACT_PLACEMENTS, TRASH_SCOPES};
use super::draft::{ActionDraft, NoticeChoice, StepDraft};

/// `before` 是 `document` 现在写着的内容，`after` 是要变成的样子（没有错的草稿）。
pub(crate) fn write_changes(
    document: &mut DocumentMut,
    before: &ActionDraft,
    after: &ActionDraft,
    types: &MimeDatabase,
) {
    let root = document.as_table_mut();
    if after.name != before.name {
        set_value(root, "name", after.name.trim().into());
    }
    if after.types != before.types {
        let texts = after
            .types
            .texts(types)
            .expect("only drafts without mistakes are written");
        if texts.is_empty() {
            root.remove("types");
        } else {
            set_value(root, "types", Value::Array(type_list(&texts)));
        }
    }
    if after.steps != before.steps {
        write_steps(root, &before.steps, &after.steps);
    }
    if after.notice != before.notice {
        write_notice(root, after);
    }
}

/// 几个类型写在一行；多了（“压缩包”有十几个）一行一个，手改时好找。
fn type_list(texts: &[String]) -> Array {
    const ONE_LINE: usize = 3;
    let mut list = texts.iter().collect::<Array>();
    if texts.len() > ONE_LINE {
        for item in list.iter_mut() {
            item.decor_mut().set_prefix("\n    ");
        }
        list.set_trailing_comma(true);
        list.set_trailing("\n");
    }
    list
}

/// 换掉值，留下原来的值前后的空白和行尾注释；没有这个键就加上。原来是表（`[notify]`）时整个换掉：
/// 表头的键没有 `key = value` 的空白。
fn set_value(table: &mut dyn TableLike, key: &str, mut value: Value) {
    if let Some(Item::Value(old)) = table.get_mut(key) {
        *value.decor_mut() = old.decor().clone();
        *old = value;
        return;
    }
    table.remove(key);
    table.insert(key, Item::Value(value));
}

/// 一步在文件里是一个键：`convert = "m4a"`。写法和读文件的表是同一份。
fn step_entry(step: &StepDraft) -> (&'static str, Value) {
    match step {
        StepDraft::Convert(format) => ("convert", format.name().into()),
        StepDraft::Compress(format) => ("compress", format.extension().into()),
        StepDraft::Extract(placement) => {
            ("extract", spelling(&EXTRACT_PLACEMENTS, *placement).into())
        }
        StepDraft::CopyTo(folder) => ("copy_to", folder.text().into()),
        StepDraft::MoveTo(folder) => ("move_to", folder.text().into()),
        StepDraft::Trash(scope) => ("trash", spelling(&TRASH_SCOPES, *scope).into()),
        StepDraft::Run(command) => ("run", command.text().into()),
        StepDraft::CopyPaths => ("copy_paths", true.into()),
    }
}

/// 原文档里的步骤，和它对应的草稿配好对；搬走一个就空出一个，同样的两步各搬各的。
struct OldSteps<'a, T> {
    slots: Vec<Option<(&'a StepDraft, T)>>,
}

impl<'a, T> OldSteps<'a, T> {
    fn new(drafts: &'a [StepDraft], items: impl IntoIterator<Item = T>) -> Self {
        Self {
            slots: drafts.iter().zip(items).map(Some).collect(),
        }
    }

    fn take(&mut self, step: &StepDraft) -> Option<T> {
        self.slots
            .iter_mut()
            .find(|slot| slot.as_ref().is_some_and(|(draft, _)| *draft == step))
            .and_then(Option::take)
            .map(|(_, item)| item)
    }
}

/// 原来是 `[[steps]]` 就还写 `[[steps]]`，原来是一行的数组就还写数组；新文档用 `[[steps]]`。
fn write_steps(root: &mut Table, before: &[StepDraft], after: &[StepDraft]) {
    match root.get_mut("steps") {
        Some(Item::Value(Value::Array(array))) => {
            let mut old = OldSteps::new(before, array.iter().cloned());
            let mut steps = Array::new();
            // 搬过去的项带着自己前后的空白和注释；新项用默认的排版。
            for step in after {
                match old.take(step) {
                    Some(value) => steps.push_formatted(value),
                    None => {
                        let (key, value) = step_entry(step);
                        let mut table = InlineTable::new();
                        table.insert(key, value);
                        steps.push(table);
                    }
                }
            }
            steps.set_trailing(array.trailing().clone());
            steps.set_trailing_comma(array.trailing_comma());
            *steps.decor_mut() = array.decor().clone();
            *array = steps;
        }
        Some(Item::ArrayOfTables(tables)) => {
            // 文档按每张表的位置号排版输出。原来那些位置号按新顺序分下去，换了顺序也照新顺序写，
            // 和 [notify] 的前后也不变；多出来的新表不设位置号，跟在前一张后面。
            let positions = tables.iter().map(Table::position).collect::<Vec<_>>();
            let mut old = OldSteps::new(before, tables.iter().cloned());
            let mut steps = ArrayOfTables::new();
            for (index, step) in after.iter().enumerate() {
                let mut table = old.take(step).unwrap_or_else(|| step_table(step));
                table.set_position(positions.get(index).copied().flatten());
                steps.push(table);
            }
            *tables = steps;
        }
        _ => {
            root.insert(
                "steps",
                Item::ArrayOfTables(after.iter().map(step_table).collect()),
            );
        }
    }
}

fn step_table(step: &StepDraft) -> Table {
    let (key, value) = step_entry(step);
    let mut table = Table::new();
    table.insert(key, Item::Value(value));
    table
}

/// 默认 = 不写 `notify`；不通知 = `notify = "none"`；自己写 = `[notify]` 表。原来就是表（包括一行的表）
/// 时在原表里改，表上面的注释和其他行留着。
fn write_notice(root: &mut Table, draft: &ActionDraft) {
    let notice = &draft.notice;
    match notice.choice() {
        NoticeChoice::Standard => {
            root.remove("notify");
        }
        NoticeChoice::Silent => set_value(root, "notify", "none".into()),
        NoticeChoice::Custom => {
            // 原来是 `notify = "none"` 时先拿掉再加：加在最后，新表排在步骤后面。
            if root.get("notify").and_then(Item::as_table_like).is_none() {
                root.remove("notify");
                root.insert("notify", Item::Table(Table::new()));
            }
            let table = root
                .get_mut("notify")
                .and_then(Item::as_table_like_mut)
                .expect("notify was just made a table");
            set_value(table, "title", notice.title().text().into());
            match notice.body().value() {
                Some(Some(_)) => set_value(table, "body", notice.body().text().into()),
                _ => {
                    table.remove("body");
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "document_tests.rs"]
mod tests;
