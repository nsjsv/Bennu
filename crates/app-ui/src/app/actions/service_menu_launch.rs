//! 启动服务菜单。同 Dolphin：启动之后不管，不进任务队列，不记撤销，不发桌面通知；
//! 启动不了时在窗口里报错。

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use iced::Task;

use super::{ActionMessage, FileBrowser};
use crate::actions::service_menu::exec::{ExecValues, ShellLaunch};
use crate::actions::ServiceMenuAction;
use crate::model::Message;

impl FileBrowser {
    pub(crate) fn launch_service_menu(
        &mut self,
        definition: &ServiceMenuAction,
        items: Vec<PathBuf>,
    ) -> Task<Message> {
        let values = ExecValues {
            name: &definition.name,
            icon: &definition.icon,
            desktop_file: &definition.file,
        };
        let name = definition.name.clone();
        let launches = match definition.command.launches(&items, &values) {
            Ok(launches) => launches,
            Err(refused) => {
                self.show_global_error(tr!("%1 failed: %2", name, refused.reason()));
                return Task::none();
            }
        };
        let program = definition.command.program().map(str::to_owned);
        Task::perform(start_processes(program, launches), move |result| {
            Message::Action(ActionMessage::ServiceMenuEnded { name, result })
        })
    }
}

/// 先确认程序在（同 KIO，免得 sh 悄悄报"找不到命令"），再逐个启动。不等它们结束：
/// tokio 在后台回收退出的子进程。各自一个进程组，在终端里按 Ctrl+C 不会把它们一起带走。
async fn start_processes(
    program: Option<String>,
    launches: Vec<ShellLaunch>,
) -> Result<(), String> {
    if let Some(program) = program {
        tokio::task::spawn_blocking(move || check_program(&program))
            .await
            .map_err(|error| error.to_string())??;
    }
    for launch in launches {
        tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&launch.script)
            .arg("sh")
            .args(&launch.arguments)
            .current_dir(&launch.directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// 同 KIO 的 `resultingArguments`：名字沿 `PATH` 找，路径直接看；相对路径（`./run`）
/// 要看启动时在哪，不查。
fn check_program(program: &str) -> Result<(), String> {
    let executable = |path: &Path| {
        std::fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    };
    let candidates: Vec<PathBuf> = if program.contains('/') {
        if !program.starts_with('/') {
            return Ok(());
        }
        vec![PathBuf::from(program)]
    } else {
        std::env::var_os("PATH")
            .map(|path| {
                std::env::split_paths(&path)
                    .map(|directory| directory.join(program))
                    .collect()
            })
            .unwrap_or_default()
    };
    if candidates.iter().any(|candidate| executable(candidate)) {
        return Ok(());
    }
    match candidates.iter().find(|candidate| candidate.is_file()) {
        Some(found) => Err(tr!("The program %1 is not executable", found.display())),
        None => Err(tr!("Could not find the program “%1”", program)),
    }
}

#[cfg(test)]
#[path = "service_menu_launch_tests.rs"]
mod tests;
