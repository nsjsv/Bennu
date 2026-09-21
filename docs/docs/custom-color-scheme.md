# 自定义配色

内置配色家族之外，「自定义」家族允许你用一份 JSON 文件定义自己的浅色和深色配色，两套颜色随深浅模式自动切换。

## 导入自定义配色

1. 打开 **设置 → 外观 → 配色预设**；
2. 在家族网格中选择 **自定义**（Custom），点击 **导入 JSON**；
3. 选择写好的 JSON 文件。

导入成功后会自动选中自定义配色，并随其他偏好一起保存，重启后保留。

## JSON 格式

```json
{
  "version": 1,
  "light": {
    "background": "#f7f5f2",
    "surface": "#edeae5",
    "text": "#2b2926",
    "muted_text": "#7a756d",
    "primary": "#9a7b4f",
    "success": "#4a7c59",
    "warning": "#b08d57",
    "danger": "#a84c4c"
  },
  "dark": {
    "background": "#1b1a18",
    "surface": "#262420",
    "text": "#e8e6e1",
    "muted_text": "#a09b92",
    "primary": "#c2a374",
    "success": "#7fb891",
    "warning": "#d4b478",
    "danger": "#d47878"
  }
}
```

各字段的用途：

| 字段 | 用途 |
| --- | --- |
| `background` | 窗口背景 |
| `surface` | 卡片、面板等表面 |
| `text` | 正文文字 |
| `muted_text` | 次要文字 |
| `primary` | 主色，用于选中、焦点等状态 |
| `success` | 成功状态 |
| `warning` | 警告状态 |
| `danger` | 危险、错误操作 |

颜色值必须写成 `#RRGGBB` 格式的十六进制，例如 `#1b1a18`，不支持缩写 `#fff`。

## 校验与失败处理

导入时会严格校验：`version` 必须为 `1`，`light` 和 `dark` 两套字段必须齐全，不允许出现未知字段。任何一项不满足都会导入失败，并显示具体原因；此时当前配色和已保存的偏好完全不变，取消选择文件也同样不会有任何改动。

## 对比度提示

如果 `background/text` 或 `surface/muted_text` 的对比度低于 2.4:1，导入时会提示「自定义颜色的文字对比度较低」。这只是警告，配色仍会生效；觉得文字看不清的话，把文字颜色和背景颜色拉开差距即可。

## 深浅模式

`light` 和 `dark` 两套颜色独立保存：浅色模式使用 `light` 套，深色模式使用 `dark` 套，Automatic 则跟随启动时检测到的系统深浅。切换深浅模式时，两套配色会自动切换。

想从壁纸自动生成配色的话，见 [Matugen 配色](matugen.md)。
