<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Korterm

**Fleet 风格的独立终端模拟器** — 用 Rust + [iced](https://github.com/iced-rs/iced) 从零打造的桌面终端,带 macOS "island" 视觉、可拖拽标签页和 Quake 式快捷下拉终端。

<div align="center"><img src="assets/icon-256.png" width="128" alt="Korterm 图标"/></div>

## 特性

- **终端核心** — 内置 PTY(portable-pty)+ 自研 VT 解析器,输入回显、动态 resize、光标动画一应俱全
- **标签页** — 水平标签 / 右侧边栏两种布局;拖拽重排(FLIP 滑动动画)、双击重命名、溢出自动收纳
- **快捷终端** — `korterm --quick` 唤出 Quake 式顶部下拉终端,全局系统快捷键一键切换(X11/XWayland)
- **中文输入** — 完整 IME 支持(preedit 内联显示),Wayland 与 XWayland 下的 fcitx5 均已适配
- **搜索** — `Ctrl+F` 搜索终端缓冲区,高亮全部匹配并逐个跳转
- **Ctrl+点击跳转** — 识别 URL、文件路径与 `path:line(:col)` 位置,一键 `xdg-open`
- **设置面板** — 外观(状态栏 / 标签布局 / 侧栏宽度)、辉光颜色(HSV 取色器)、快捷键(捕获式重绑定 + 冲突检测),全部持久化
- **Fleet 辉光** — 可自定义双色径向辉光背景与 24 帧补间动画系统(按需 60fps,空闲零唤醒)

## 平台支持

> [!NOTE]
> Korterm 目前是 **Linux 专用**(X11 / XWayland)。快捷终端依赖 X11 的窗口定位与置顶能力。

## 安装

### 从源码安装(推荐)

```bash
git clone <repo-url>
cd Korterm
./install.sh              # 构建并安装到 ~/.local
./install.sh /usr/local   # 或安装到其他前缀
./install.sh --uninstall  # 卸载
```

安装内容包括:`korterm` 可执行文件、全尺寸 hicolor 图标、`.desktop` 桌面入口。

### Debian / Ubuntu(.deb)

```bash
cargo install cargo-deb
cargo deb                 # 生成 target/debian/korterm_1.0.0-1_amd64.deb
```

### 手动构建

```bash
cargo build --release     # 产物: target/release/korterm
```

构建依赖:`libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libxcb-cursor-dev`(及常规 X11 开发库)。

## 使用

### 主程序

直接运行 `korterm`。无边框窗口、44px 自定义标题栏、红绿灯按钮,标签页常驻标题栏(或侧边栏)。

### 快捷终端(Quake 模式)

把桌面环境的系统快捷键绑定到:

```bash
korterm --quick
```

按一次唤出(顶部半屏、置顶、淡入),再按一次收起。首次运行会自动安装 `.desktop` 条目并引导你打开系统快捷键设置。

### 默认快捷键

| 操作 | 快捷键 |
|---|---|
| 复制 / 粘贴 | `Ctrl+Shift+C` / `Ctrl+Shift+V` |
| 搜索终端 | `Ctrl+F` |
| 快速终端 | ``Ctrl+` `` |
| 新建终端 | `Ctrl+Shift+T` |
| 关闭当前终端 | `Ctrl+Shift+W` |
| 清除终端 | `Ctrl+Shift+K` |
| 下一个 / 上一个标签 | `Ctrl+Shift+→` / `Ctrl+Shift+←` |
| 打开设置 | `Ctrl+Shift+S` |

所有快捷键都可在设置面板中改绑。

## 配置

配置文件位于 `~/.config/korterm/config.conf`,格式为简单的 `key = value` 行(未知键忽略,损坏值回退默认):

```ini
statusbar_visible = true
tabs_vertical = false
sidebar_width = 180.0
shell = zsh
glow_blue = 0x4a8cff
glow_amber = 0xc99a5b
glow_intensity_top = 1.00
glow_intensity_bottom = 1.00
keybind_copy = ctrl+shift+c
```

> [!TIP]
> 旧版(`kortina-terminal`)的配置会在首次运行新版时自动迁移,无需手动处理。

## 开发

```bash
cargo check        # 快速检查
cargo test         # 运行 28 个单元测试
cargo clippy       # lint(当前零警告)
```

### 代码结构

| 模块 | 职责 |
|---|---|
| `terminal_panel.rs` | 核心状态机:会话管理、PTY 泵、输入路由、标签与菜单 |
| `quick.rs` | Quake 式快捷终端(独立进程,socket 单实例 toggle) |
| `settings.rs` | 设置面板(外观 / 辉光 / 快捷键 / 关于) |
| `titlebar.rs` | 44px 自定义标题栏与标签页 |
| `keybinds.rs` | 快捷键定义、匹配与默认键位 |
| `config.rs` | 配置持久化与旧版迁移 |
| `animation.rs` | cubic-bezier 缓动、FLIP 补间、IME 输入区 |
| `glow.rs` / `theme.rs` / `styles.rs` / `icons.rs` | 视觉层 |

终端模拟与 PTY 由姊妹项目 [`Kortina.ICED/terminal`](../Kortina.ICED/terminal) 提供。

## 许可证

本项目以 [MPL-2.0](LICENSE)(Mozilla Public License v2.0)发布。
