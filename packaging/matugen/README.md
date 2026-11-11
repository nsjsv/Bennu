# Matugen 配色

Bennu 会读取 `$XDG_CONFIG_HOME/bennu/matugen.toml`（默认 `~/.config/bennu/matugen.toml`），并在文件变化时热更新所有已打开窗口。在设置 → 外观 → 配色里选“Matugen”才会使用它。一次生成会同时写入浅色和深色两套，Bennu 按设置里的深浅模式选用。Matugen 是可选依赖；没有生成文件时，Matugen 配色和默认配色一样。

> 旧版本模板生成的文件（`version = 1`）不再支持，请用新模板重新生成一次。

发行包中的模板位于：

```text
/usr/share/bennu/matugen/bennu-colors.toml
```

源码仓库中的模板位于 `packaging/matugen/bennu-colors.toml`。把下面内容合并到 `~/.config/matugen/config.toml`；已有 `[config]` 时不要重复添加该表：

```toml
[config]

[templates.bennu]
input_path = "/usr/share/bennu/matugen/bennu-colors.toml"
output_path = "~/.config/bennu/matugen.toml"
```

## DMS 自动更新

DMS 会把 `~/.config/matugen/config.toml` 中的用户模板合并进每次主题生成。确认 DMS 的 **Run user Matugen templates** 已启用后，上面的 `templates.bennu` 会在切换壁纸或深浅模式时自动生成 Bennu 配色；不需要 `post_hook`。

源码调试且尚未安装发行包时，可把 `input_path` 改为仓库模板的绝对路径，或先复制模板：

```bash
install -Dm644 packaging/matugen/bennu-colors.toml \
  ~/.config/matugen/templates/bennu-colors.toml
```

然后使用：

```toml
[templates.bennu]
input_path = "~/.config/matugen/templates/bennu-colors.toml"
output_path = "~/.config/bennu/matugen.toml"
```

## 直接使用 Matugen

创建输出目录后生成一次配色（浅色、深色两套一起写入，`-m` 选哪个都行）：

```bash
mkdir -p ~/.config/bennu
matugen image ~/Pictures/wallpaper.jpg
```

不需要配置 `post_hook`。有效输出会直接替换当前配色；删除输出文件后 Matugen 配色回到和默认配色一样；短暂或畸形写入会被忽略并保留上一套有效配色。
