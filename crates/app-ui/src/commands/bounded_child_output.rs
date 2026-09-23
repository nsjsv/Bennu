// 受限子进程输出工具已下沉共享 crate bennu-subprocess（预览命令层与
// journalctl/systemctl 查询共同消费，单一实现）；此处 re-export 维持
// `crate::commands::bounded_child_output::*` 既有路径，三个消费者零改动。
pub(crate) use bennu_subprocess::{read_bounded_child_output, BoundedChildOutput};
