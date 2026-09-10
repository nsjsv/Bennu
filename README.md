# Bennu

[![AUR](https://img.shields.io/aur/version/bennu-bin)](https://aur.archlinux.org/packages/bennu-bin)
[![License](https://img.shields.io/github/license/nsjsv/Bennu)](LICENSE)
[![Telegram](https://img.shields.io/badge/Telegram-群聊-26A5E4?logo=telegram&logoColor=white)](https://t.me/bennu_chat)

**一个认真对待文件操作的 Linux 文件管理器**

用 Rust 编写，围绕多栏浏览、后台任务、索引搜索与内容预览构建。

主要在 Wayland 环境下开发和测试。

[Releases](https://github.com/nsjsv/Bennu/releases) · [AUR](https://aur.archlinux.org/packages/bennu-bin) · [Issues](https://github.com/nsjsv/Bennu/issues) · [Pull Requests](https://github.com/nsjsv/Bennu/pulls) · [License](LICENSE)

## 功能亮点

- **多栏浏览，也保留熟悉的列表视图。** 沿目录层级逐栏展开，或切换到更适合扫描和排序的列表视图。标签页与分屏面板可以同时处理多个位置。
- **搜索范围由你决定。** 可以递归搜索当前目录、Home 或所有已索引位置，也可以在设置中加入自定义索引目录。后台服务不可用时，当前目录搜索仍会回退到本地扫描。
- **先看一眼，不必每次都打开应用。** 可预览文本、Markdown、目录树、压缩包、PDF、Office 文档、图片（含 AVIF）、音频和视频。部分格式需要安装可选依赖。
- **融入 Linux 桌面，而不是另起一套。** 支持 GTK 书签、UDisks2 存储设备、SMB / WebDAV / SFTP 网络位置、桌面主题、默认应用与终端调用。
- **常用细节可以自己定。** 隐藏文件、启动位置、会话恢复、语言、终端、GPU 偏好、文件校验和快捷键都可以在设置中调整。属性窗口支持目录统计和权限修改。

此外还支持回收站、批量重命名、压缩与解压、拖放，以及多目标文件属性等日常操作。

## 安装



### Arch Linux（AUR）

推荐直接安装预编译包 [bennu-bin](https://aur.archlinux.org/packages/bennu-bin)：

```bash
paru -S bennu-bin
```

> 从旧包 `file-manager-bin` 迁移：直接安装 `bennu-bin`，pacman 会提示移除旧包；
> 首次启动会自动把 `~/.local/share/file-manager` 与 `~/.config/file-manager`
> 搬移到 `bennu/`，标签页、偏好与操作记录原样保留。

### Debian / Ubuntu

从 [Releases](https://github.com/nsjsv/Bennu/releases) 下载 `.deb` 安装（兼容 Ubuntu 24.04+、Debian 13+）：

```bash
sudo apt install ./bennu_0.2.6-1_amd64.deb
```

### Fedora / openSUSE

从 [Releases](https://github.com/nsjsv/Bennu/releases) 下载 `.rpm` 安装：

```bash
sudo dnf install ./bennu-0.2.6-1.*.rpm
```

其他发行版可以先查看 [Releases](https://github.com/nsjsv/Bennu/releases)。如果预编译包无法运行，请按下文从源码构建。

以上预编译包均不自动启用后台服务，PDF/Office、视频、压缩包等预览依赖的可选软件（poppler、ffmpeg、7zip、gvfs 等）以弱依赖声明，需要时自行安装。

### 启用索引搜索

发行包会安装 `bennu-search.service`。启用后，搜索服务会在后台维护 Home 和自定义位置的索引：

```bash
systemctl --user enable --now bennu-search.service
```

查看状态和日志：

```bash
systemctl --user status bennu-search.service
journalctl --user -u bennu-search.service -f
```



## 使用

安装后的命令名是 `bennu`：

```bash
bennu
```


| 命令                                     | 行为                        |
| -------------------------------------- | ------------------------- |
| `bennu`                         | 按设置中的 Home、自定义目录或上次会话策略启动 |
| `bennu .`                       | 打开当前目录                    |
| `bennu ~/Downloads ~/Documents` | 按顺序在同一窗格中打开多个标签           |
| `bennu report.pdf notes.txt`    | 打开文件所在目录并选中对应文件           |


应用已经运行时，再次执行命令会复用现有实例：不带路径时聚焦主窗口，带路径时复用已有标签并补充缺失目录，不会清空其他标签或分栏。

路径可以是绝对路径，也可以相对当前终端目录。当前 CLI 只接受本地文件系统路径，不接受 URI；如果任一路径无效，整次启动都会被拒绝。

查看帮助和版本：

```bash
bennu --help
bennu --version
```

## 文档

更多使用教程在 `docs/` 目录：

- [Matugen 配色](docs/matugen.md)：从壁纸生成配色，所有窗口热更新
- [Niri 悬浮窗口](docs/niri.md)：让设置、属性、预览窗口悬浮打开
- [自定义配色](docs/custom-color-scheme.md)：用 JSON 定义浅色与深色配色



## 从源码构建

下面以 Arch Linux 为例。先安装 Rust 工具链、构建依赖和核心运行命令：

```bash
sudo pacman -S --needed base-devel git rust cargo pkgconf \
  acl alsa-lib dav1d fontconfig glib2 libnotify libxkbcommon wayland wl-clipboard xdg-utils
```

克隆仓库并运行：

```bash
git clone https://github.com/nsjsv/Bennu.git
cd Bennu
cargo run --locked -p app-ui
```

构建 release 版 App 和搜索 daemon：

```bash
cargo build --release --locked -p app-ui -p file-search
./target/release/app-ui
```

源码构建的 App 二进制名为 `app-ui`，搜索 daemon 为 `bennu-searchd`。这些命令不会安装 systemd user unit 或品牌 D-Bus service；如果系统中没有兼容的搜索服务，索引搜索不可用，但当前目录搜索仍可回退到本地扫描。

**可选预览、压缩包与桌面集成依赖**

```bash
sudo pacman -S --needed 7zip ffmpeg ffmpegthumbnailer libreoffice-fresh poppler \
  gvfs gvfs-afc gvfs-gphoto2 gvfs-mtp gvfs-smb libsecret udisks2
```


| 能力                               | 依赖                                    |
| -------------------------------- | ------------------------------------- |
| 视频预览与元数据                         | `ffmpeg`；视频缩略图可使用 `ffmpegthumbnailer` |
| PDF 预览                           | Poppler（`pdfinfo`、`pdftoppm`）         |
| Office 文档预览                      | LibreOffice 与 Poppler                 |
| `.7z` 创建，以及 `.7z` / `.rar` 预览和解压 | `7z`、`7zz` 或 `7za`（Arch 包名为 `7zip`）   |
| SFTP、WebDAV                      | `gvfs`                                |
| SMB                              | `gvfs-smb`                            |
| Android / MTP 设备                 | `gvfs-mtp`                            |
| 数码相机                             | `gvfs-gphoto2`                        |
| Apple / AFC 设备                   | `gvfs-afc`                            |
| 保存网络密码                           | `libsecret` 提供的 `secret-tool`         |
| 存储设备发现、挂载与安全移除                   | `udisks2`                             |


缺少可选依赖时，只会影响对应能力。AVIF 由内置图片解码器支持，构建和运行时使用 `dav1d`。

## 平台说明

目前主要支持 Linux / Wayland。其他发行版如果无法使用预编译包，建议从源码构建。

Windows、macOS 和 X11 不保证可用。

**D-Bus 与其他文件管理器共存**

运行中的应用会以 `DoNotQueue` 尝试提供标准 `org.freedesktop.FileManager1` 接口。如果 Nautilus、Dolphin、Thunar 等文件管理器已经占用标准名称，本应用不会替换它，也不会排队抢占；品牌单实例端点和普通 GUI 仍可正常使用。

发行包不会安装另一个声明标准名称的 D-Bus service 文件。

## 参与项目

如果某个操作不符合你的习惯，或在特定发行版、桌面环境中无法工作，欢迎提交 [Issue](https://github.com/nsjsv/Bennu/issues)。提供复现步骤、预期结果、实际结果和相关日志，会让问题更容易定位。

代码改动可以通过 [Pull Request](https://github.com/nsjsv/Bennu/pulls) 提交。

### AI 辅助提交说明

如果 PR 中的代码主要由 AI 生成，或经过 AI 大量辅助修改，请在标题中加上 `[AI]`：

```text
[AI] 修复文件搜索刷新问题
```

只用 AI 做少量解释、资料查询或辅助排查时，不需要添加该标识。

## 许可证

本项目采用 [GPL-3.0-or-later](LICENSE) 许可证。

## 致谢

感谢 [LinuxDo](https://linux.do/) 社区的交流与反馈。
