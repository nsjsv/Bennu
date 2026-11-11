# Matugen 配色

[Matugen](https://github.com/InioX/matugen) 可以从壁纸生成配色。Bennu 读取 Matugen 生成的配色文件：换了壁纸，文件管理器跟着换色。

## 工作方式

Bennu 读取 `$XDG_CONFIG_HOME/bennu/matugen.toml`（默认 `~/.config/bennu/matugen.toml`），文件变化时所有已打开的窗口立即换色，不需要重启。

一次生成同时写入浅色和深色两套，每套只有底色、文字色和强调色，其余层次由 Bennu 自己算。深浅模式仍按 **设置 → 外观 → 主题** 里的选择（跟随系统时随系统切换）。

配色只在你于 **设置 → 外观 → 配色** 中选择 **Matugen** 后生效。Matugen 是可选依赖：没有生成文件时，Matugen 配色和默认配色一样。

> 旧版本模板生成的文件（`version = 1`）不再支持，用新模板重新生成一次即可。

## 启用步骤

1. 在 **设置 → 外观 → 配色** 中选择 **Matugen**；
2. 把下面的模板配置合并进 `~/.config/matugen/config.toml`（已有 `[config]` 时不要重复添加该表）：

```toml
[config]

[templates.bennu]
input_path = "/usr/share/bennu/matugen/bennu-colors.toml"
output_path = "~/.config/bennu/matugen.toml"
```

3. 生成一次配色（浅色、深色两套一起写入，`-m` 选哪个都行）：

```bash
mkdir -p ~/.config/bennu
matugen image ~/Pictures/wallpaper.jpg -m dark --prefer saturation
```

### 从源码构建

尚未安装发行包时，发行包模板路径不存在，可以把 `input_path` 指向源码仓库里的模板，或先复制一份再引用：

```bash
install -Dm644 packaging/matugen/bennu-colors.toml \
  ~/.config/matugen/templates/bennu-colors.toml
```

```toml
[templates.bennu]
input_path = "~/.config/matugen/templates/bennu-colors.toml"
output_path = "~/.config/bennu/matugen.toml"
```

## 配合 DMS 自动更新

DMS 会把 `~/.config/matugen/config.toml` 中的用户模板合并进每次主题生成。确认 DMS 的 **Run user Matugen templates** 已启用后，上面的 `templates.bennu` 会在切换壁纸或深浅模式时自动重新生成 Bennu 配色，不需要配置 `post_hook`。

## 行为说明

- 不需要配置 `post_hook`，有效输出会直接替换当前配色；
- 删除输出文件后，Matugen 配色回到和默认配色一样；
- 短暂或畸形的写入会被忽略，并保留上一套有效配色；
- 没选 Matugen 时，文件更新只记下新颜色，当前界面不换色。

模板见 `/usr/share/bennu/matugen/bennu-colors.toml`（源码仓库中为 `packaging/matugen/bennu-colors.toml`）。

手动生成时若壁纸包含多个候选源色，matugen 会拒绝猜测并退出，`--prefer saturation` 让它直接选饱和度最高的源色（可选值见 `matugen image --help`）。DMS 自动触发时自带偏好参数，不受影响。
