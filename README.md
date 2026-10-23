<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="96" alt="AgentDock" />

# AgentDock

**你的个人 AI agent 语音通讯录 —— 一条悬浮在桌面上的 Liquid Glass 拨号盘。**

把 ChatGPT dot、Grok Bot 里的每个 Bot、Copilot、Devin、Muse 当成一排可呼叫的联系人：<br/>
一键呼叫 · 一键闭麦 · 一键挂断或切到下一位。不是聊天窗口，不是模型路由器。

[![macOS 26 Liquid Glass](https://img.shields.io/badge/macOS%2026-Liquid%20Glass-0a84ff?logo=apple&logoColor=white)](#平台) [![Windows Mica](https://img.shields.io/badge/Windows%2011-Mica%20(未实机验证)-6b7280?logo=windows&logoColor=white)](#平台) [![Tauri 2](https://img.shields.io/badge/Tauri-2-24c8db?logo=tauri&logoColor=white)](https://tauri.app) [![Rust](https://img.shields.io/badge/Rust-core-dea584?logo=rust&logoColor=black)](src-tauri) [![Tests](https://img.shields.io/badge/tests-56%20unit%20%C2%B7%2031%20e2e-2ea043)](#测试) [![CI](https://github.com/we1jia/AgentDock/actions/workflows/ci.yml/badge.svg)](https://github.com/we1jia/AgentDock/actions/workflows/ci.yml) [![Release](https://img.shields.io/github/v/release/we1jia/AgentDock?label=下载)](https://github.com/we1jia/AgentDock/releases/latest) [![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

[English](README.en.md) · [功能](#功能) · [安装](#安装) · [快捷键](#快捷键) · [架构](#架构) · [接入席位](#接入席位) · [长期记忆](#长期记忆everos) · [隐私](#隐私与边界) · [路线图](#路线图)

<br/>

<img src="docs/screenshots/mac-hero-light.png" width="920" alt="AgentDock 空闲态胶囊：收起、闭麦、九位联系人、添加、方向把手" />
<br/>

<p><sub>macOS 26 真机截图 · 系统 Liquid Glass（NSGlassEffectView）。胶囊从左到右：收起 · 系统闭麦 · 联系人（Grok Bot 里的三个 Bot · Muse · dot · Gemini · Claude · Antigravity · Cursor，后四个暂为待接入占位）· 添加 · ⇅ 方向把手</sub></p>

</div>

---

## 为什么做它

每家 AI 都有了自己的「打电话」：ChatGPT 的 dot、xAI 的 Grok Bot、Copilot Voice、Meta Muse、Devin。但它们散落在五个 App 和网页里，没有一个地方能像手机通讯录那样**一键呼叫、随手换人**。

AgentDock 做的事情很窄：

- **每位 agent 是一个席位**，不是一个模型。你登录你自己的账号，AgentDock 只负责按下「通话 / 静音 / 挂断」。
- **一条胶囊**，灵动岛式三级形态（小圆点 / 宠物 → 胶囊 → 通话态），玻璃由系统渲染。
- **通话中可以直接切到另一位**：当前通话挂断、下一位接通，3 秒内可撤销。
- **系统级闭麦**：一键对所有席位生效，不依赖各家网页里的静音按钮。

## 功能

<table>
<tr>
<td width="50%" valign="top">

**呼叫与切换**

- 点席位呼叫；通话中点另一位 = 切换（3 秒可撤销）
- 一键闭麦 / 开麦（系统输入设备级）、一键挂断
- 15 秒未接通自动进入「需要你看一眼」，可一键查看该席位的后台页面
- ⌥1…⌥9 直接呼叫 / 切换到第 n 席

**形态**

- 小圆点 / 宠物（席位头像，可用 GIF / APNG）/ 胶囊 / 通话态
- 空闲自动收起（3 / 8 / 15 / 30 秒 / 从不），鼠标停在圆点 0.3 秒自动展开
- 收起方式三档：自动 · 固定展开 · 固定收起（hover 只预览）
- 通话中永不自动收起

</td>
<td width="50%" valign="top">

**位置与方向**

- 按住胶囊玻璃处 / 小圆点 / 末端把手拖到任意位置，松手即记忆
- 横向（Spotlight 式）/ 竖向（侧边 Dock 式）一键切换
- 末端把手：横向显示 ⇅、竖向显示 ⇄，点一下切方向、按住拖动
- 横竖切换与展开收起都围绕同一中心形变，不跳位

**系统集成**

- macOS 26 `NSGlassEffectView`（原生 Liquid Glass），老系统回退 `NSVisualEffectView`
- 全局快捷键可录制、冲突检测；原生右键菜单
- 浅色 / 深色 / 跟随系统，原生玻璃层随之同步
- 旧版 `settings.json` 向后兼容

</td>
</tr>
<tr>
<td width="50%" valign="top">

**接入与登录**

- 网页版 Agent 直接用你 Chrome 里的登录（Chrome 远程调试），不用再登一次；没开时退回内置浏览器
- 桌面 App（Grok Bot、ChatGPT）通过辅助功能在后台按按钮，不切到前台
- Grok Bot 里每个 Bot 一个联系人：自动读出 Bot 列表，群聊（不能语音）自动排除
- 同一个 App 一次只能一路通话：切换时先挂断、等通话真正结束，再拨下一位

</td>
<td width="50%" valign="top">

**应用与设置**

- 界面语言：简体中文 · English · 繁體中文 · 日本語，默认跟随系统；设置窗、Dock、右键与菜单栏菜单、呼叫失败提示一起切换
- 程序坞图标点一下打开设置；可改为仅程序坞或仅菜单栏
- 「添加联系人」目录：每个服务一行，标明网页 / App，没装的 App 置灰
- 联系人拖拽排序（= ⌥1…⌥9）、开关、改名、换图标（支持 GIF）
- 本机固定签名打包，重新打包后辅助功能授权不丢

</td>
</tr>
</table>

## 截图

### macOS 26 真机

> 文件名 `mac-*`，Apple Silicon，玻璃由系统 `NSGlassEffectView` 绘制。设置页上那颗圆点是收起后的 Dock，它浮在所有窗口上面。

<div align="center">

| 竖向 | 收起：小圆点 · 右键原生菜单（标注当前快捷键） |
|:--:|:--:|
| <img src="docs/screenshots/mac-vertical-idle-light.png" height="520" /> | <img src="docs/screenshots/mac-dot-light.png" width="120" /><br/><br/><img src="docs/screenshots/mac-menu-light.png" width="280" /> |

<br/>

| 设置 › Agent（展开的胶囊浮在上面） | 设置 › 记忆（EverOS） |
|:--:|:--:|
| <img src="docs/screenshots/mac-settings-agents-light.png" width="460" /> | <img src="docs/screenshots/mac-settings-memory-light.png" width="460" /> |
| **设置 › 快捷键** | **设置 › 麦克风** |
| <img src="docs/screenshots/mac-settings-hotkeys-light.png" width="460" /> | <img src="docs/screenshots/mac-settings-mic-light.png" width="460" /> |

</div>

## 安装

### 直接下载（macOS）

到 [Releases](https://github.com/we1jia/AgentDock/releases/latest) 下载 `AgentDock_<版本>_universal.dmg`（Apple 芯片和 Intel 通用），把 AgentDock 拖进「应用程序」。

安装包没有经过 Apple 公证（ad-hoc 签名），第一次打开会被拦下：到 **系统设置 › 隐私与安全性** 底部点「仍要打开」，或在终端执行一次 `xattr -dr com.apple.quarantine /Applications/AgentDock.app`。

### 从源码构建（macOS，首发平台）

```bash
# 工具链（已装可跳过）
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
brew install node

git clone https://github.com/we1jia/AgentDock.git && cd AgentDock
npm install
npm run tauri dev        # 开发模式（热更新）
npm run tauri build      # 打包 .app / .dmg → src-tauri/target/release/bundle/
```

macOS 本机打包建议用固定签名：未签名（ad-hoc）的包每次重新打包签名都会变，系统会收回辅助功能等授权，Grok Bot 就得重新授权。

```bash
npm run signing          # 一次性：在登录钥匙串里创建自签名证书「AgentDock Local Signing」
npm run build:mac        # 用这个证书签名打包，授权在重新打包后依然有效
```

首次启动：程序坞和菜单栏都出现 AgentDock 图标，屏幕顶部中央出现一个小圆点。点程序坞图标打开设置，`⌥Space` 展开，右键胶囊打开菜单 / 设置。设置 › 外观 › 应用图标 可改为「仅程序坞」或「仅菜单栏」。

| 要求 | 版本 |
|---|---|
| macOS | 26 Tahoe 起为原生 Liquid Glass；13+ 回退到 NSVisualEffectView |
| Rust | stable（1.80+） |
| Node | 20+ |

### Windows

代码路径已就位（Mica 材质、UI Automation 驱动、`IAudioEndpointVolume` 闭麦），**尚未在 Windows 实机编译验证**。欢迎 PR。

### Linux

可编译运行（WebKitGTK，CSS 玻璃回退）。主要用于开发与 CI：`bash tests/e2e_xvfb.sh`。

## 快捷键

全局，均可在 **设置 › 快捷键** 重新录制；右键胶囊 / 小圆点的原生菜单会显示当前绑定。

| 快捷键 | 动作 |
|---|---|
| `⌥ Space` | 展开 / 收起 Dock（通话中也可用） |
| `⌥ M` | 闭麦 / 开麦（系统级） |
| `⌥ Esc` | 挂断 |
| `⌥ O` | 横向 ↔ 竖向 |
| `⌥ P` | 收起形象：小圆点 ↔ 宠物 |
| `⌥ 1…9` | 呼叫 / 切换到第 n 席（顺序 = 设置里的拖拽顺序） |

鼠标：

| 操作 | 效果 |
|---|---|
| 点席位 | 呼叫；通话中 = 切换 |
| 点末端把手 ⇅ / ⇄ | 横竖切换 |
| 按住胶囊玻璃处 / 小圆点 / 把手拖动 | 移动 Dock，松手记忆位置 |
| 鼠标停在小圆点 0.3 秒 | 自动展开 |
| 右键 | 原生菜单：展开/收起 · 方向 · 小圆点/宠物 · 设置 · 退出 |

## 架构

```
Tauri 2
├─ Dock 窗口（透明 · 无边框 · 不抢焦点）        src/dock.ts + dock.css
├─ 设置窗口（透明）                              src/settings.ts + settings.css
│      │ invoke / events
└─ Rust core
   ├─ state.rs     状态机 Idle → Calling → InCall → Switching(3s 可撤销) | NeedsHuman
   ├─ seats.rs     席位模型 + seats.json
   ├─ settings.rs  外观 / 位置 / 方向 / 收起策略 / 快捷键 / 记忆开关 → settings.json
   ├─ hotkeys.rs   全局快捷键 + 冲突检测
   ├─ mic.rs       系统级闭麦
   ├─ everos.rs    EverOS HTTP 客户端（add / search / flush）
   ├─ memory.rs    呼叫生命周期上的记忆读写（纯逻辑，可单测）
   ├─ drivers/     驱动即数据（JSON）· chrome.rs 你的 Chrome · web.rs 内置 webview · app.rs Accessibility
   └─ lib.rs       命令 · 托盘 / 程序坞 · 原生材质 · 光标 hover 轮询 · 探测循环 · 驱动串行队列 · 记忆调度
          │
          ├─ NSGlassEffectView / NSVisualEffectView / Mica   （系统材质）
          ├─ Chrome（远程调试，每个 Agent 一个后台标签页）    （沿用 Chrome 登录）
          ├─ 隐藏 WKWebView / WebView2                        （没有 Chrome 时的回退）
          ├─ 桌面 App 的辅助功能树（Grok Bot / ChatGPT）     （后台按按钮）
          └─ 系统输入设备音量 / IAudioEndpointVolume / pactl  （闭麦）
```

**设计原则**

- **窗口即玻璃形状。** Dock 窗口随状态形变并自适应内容大小；材质、圆角、阴影由系统绘制，前端只负责布局与符号。
- **驱动即数据。** 每家的 URL、登录判定、按钮选择器、呼叫 / 挂断 / 静音步骤都在 [`src-tauri/drivers/*.json`](src-tauri/drivers)；放一份更高 `version` 的到用户目录即可热更新，无需重新编译。
- **无感接入。** 网页型席位优先跑在你 Chrome 的后台标签页里，直接沿用 Chrome 的登录；没有 Chrome 时退回隐藏、常驻的内置 webview。桌面 App 型席位通过辅助功能在后台操作，不抢前台。
- **一次一路通话。** 呼叫、挂断、闭麦在同一条驱动队列里依次执行；切换时先挂断上一位并等通话真正结束，再拨下一位。
- **不碰凭据。** 登录窗就是真实的站点登录页，由你本人完成（含 2FA / Passkey）；AgentDock 不读取、不存储密码。

```
src-tauri/src
├── state.rs        纯状态机（含撤销切换）
├── seats.rs        席位模型与持久化
├── settings.rs     外观 / 位置 / 方向 / 收起策略 / 快捷键 / 麦克风 / 记忆开关
├── hotkeys.rs      全局快捷键绑定与冲突检测
├── mic.rs          系统级闭麦（macOS / Windows / Linux）
├── everos.rs       EverOS v2 HTTP 客户端（密钥只来自环境变量）
├── memory.rs       呼叫开始时检索、接通 / 对白 / 挂断时写入
├── drivers/        mod.rs 注册表 · chrome.rs Chrome 远程调试 · web.rs 内置 webview · app.rs 桌面 App（Accessibility + Bot 列表）
└── lib.rs          Tauri 命令、托盘 / 程序坞、系统材质、hover 轮询、探测循环、驱动队列、记忆调度
src-tauri/drivers/*.json   chatgpt-dot · chatgpt-app · grok-bot · copilot-voice · muse-voice · devin-voice（+ 占位：gemini-app · claude-app · antigravity · cursor）
scripts/setup-signing.sh   一次性创建本机自签名证书，用于 npm run build:mac
src/dock.ts + dock.css     Dock 条
src/settings.ts + .css     设置窗（席位 / 快捷键 / 外观 / 麦克风 / 记忆 / 关于）
src/brand.ts               内置品牌标识；public/brand/<key>.svg 可覆盖
tests/                     e2e_mock.py（浏览器 E2E）· e2e_xvfb.sh（Linux 真实二进制冒烟）
docs/ref/                  视觉参考图与还原规格（SPEC.md）
```

## 接入席位

| 席位 | 方式 | 状态 |
|---|---|---|
| Grok Bot（每个 Bot 一个联系人） | 桌面 App · macOS 辅助功能 | 真机已接通；Bot 列表读自 App 本机缓存，群聊排除 |
| ChatGPT dot · App | 桌面 App · macOS 辅助功能 | 按钮名取自安装包（发起通话 / 挂断），待真机验证 |
| ChatGPT dot · 网页 | Chrome 标签页或内置 webview · chatgpt.com | 选择器待真机钉定 |
| Copilot Voice | Chrome 标签页或内置 webview · copilot.microsoft.com | 选择器待真机钉定 |
| Muse | Chrome 标签页或内置 webview · muse.ai | 选择器待真机钉定 |
| Devin | Chrome 标签页或内置 webview · app.devin.ai | 选择器待真机钉定 |
| Gemini · Claude · Antigravity · Cursor | 桌面 App | 先上图标占位（显示「待接入」），语音呼叫后续接入 |

**用 Chrome 里已有的登录（推荐）**

在 Chrome（144+）打开 `chrome://inspect/#remote-debugging` 开启远程调试。设置 › Agent › 「用 Chrome 的登录」默认开启：网页版 Agent 会在你 Chrome 里各开一个后台标签页，直接沿用 Chrome 的登录、Cookie 与 Passkey，不用再登录一次。第一次连接时 Chrome 会弹「允许远程调试？」，点允许；之后连接一直保持，Chrome 顶部会显示「正受到自动测试软件的控制」。你自己的标签页不会被动。没开远程调试时自动退回内置浏览器。

**首次连接（需要你本人操作一次）**

1. 设置 › 席位 › **登录 / 连接账号**：用 Chrome 时会在 Chrome 里打开该 Agent 的标签页；否则弹出真实登录窗（内置浏览器这一次可见），像平时一样登录，完成后自动收起，状态变为「已连接」。
2. 系统弹一次「允许 AgentDock 使用麦克风」，允许。
3. 点 **测试呼叫**：接通 → 2 秒 → 挂断。若提示「没检测到通话」，按下面步骤钉一次选择器。
4. Grok Bot：点 **授权辅助功能 →** 跳到系统设置勾选 AgentDock。之后在后台按 Grok Bot 里的按钮，不会把它切到前台。Grok Bot 没有网页版，只能这样接。
   - Grok Bot 窗口不在眼前几分钟后会被系统降频，按钮要隔很久才响应。所以 AgentDock 会带上关闭后台降频的启动参数（`--disable-background-timer-throttling` 等）来启动它；发现它没带这些参数在运行时（你手动打开的，或它自动更新后重启的），会等你离开 Grok Bot 两分钟以上、且没在通话时，自动重启它一次。
   - 万一按下「发起语音聊天」后没反应，AgentDock 会用 Grok Bot 自己的链接把它短暂唤到前台，接通后再切回你原来的 App。
5. 一个 Bot 一个联系人：设置 › Agent 列表底部点 **添加联系人**，Grok Bot 下会列出你所有的 Bot，点名字即加到 Dock。列表读自 Grok Bot 本机缓存，不用打开 App；群聊不能语音、侧栏里隐藏的 Bot 也点不到，这两类不会出现。呼叫时先在侧栏选中它，再按「发起语音聊天」；已有的联系人可在 ⋯ 菜单里「指定 Bot…」。
6. 在两个 Bot 之间切换：Grok Bot 一次只能一路通话，AgentDock 会先挂断当前这位、等通话结束，再拨下一位。如果 App 里还挂着一通 AgentDock 不知道的通话，也会先把它结束。
7. ChatGPT 装了桌面 App 时，在「添加联系人」里选 **ChatGPT dot · App**：用 App 里 dot 的「发起通话 / 挂断」，不走网页、不用再登录，同样只需要辅助功能权限。

**钉定选择器（每家做一次）**

驱动 JSON 里的 CSS 选择器按各站公开界面的 `aria-label` 编写，标记为 `"verified": false`，站点改版后也用同样方法更新：

1. 设置 › 席位 › ⋯ › **查看后台页面**，打开该站的隐藏页面；
2. 右键 › 检查元素，找到通话 / 静音 / 挂断按钮的稳定属性；
3. 改 `src-tauri/drivers/<id>.json`，或放一份更高 `version` 的到
   `~/Library/Application Support/ai.we1jia.agentdock/drivers/`（无需重新编译），再点「测试呼叫」。

## 长期记忆（EverOS）

AgentDock 本身不跑模型，席位是外面的语音产品。记忆因此挂在**呼叫生命周期**上，而不是挂在某一次本地生成之前：

| 时机 | EverOS | 做什么 |
|---|---|---|
| 开始呼叫（含切换生效） | `POST /api/v2/memory/search` | `hybrid`、`top_k` 默认 5，带上用户画像。命中的 episode 叙事会写进**网页席位**的输入框（标记 `[AgentDock memory]`），不自动发送 |
| 探测到已接通 | `POST /api/v2/memory/add` | 记一条「开始与某席位语音通话」 |
| 通话中约每 2 秒 | 同上 | 从页面上带角色的气泡（`data-message-author-role` 等）抓取新的对白；认不出角色的界面文字会丢掉 |
| 挂断、对方挂断、或切换生效 | `add`，约 2 秒后再 `POST /flush` | 记下时长或「没有接通」。`flush` 返回 `no_extraction` 只表示缓冲区里还没有可提取的内容，不是失败 |

每次呼叫一个 session（`ad-<席位>-<unix ms>`）。用户是 `user_id`，席位发言人是 `seat-<席位 id>`。`app_id` 默认 `agentdock`，避免和同一把 key 下的其他应用串记忆。写入默认异步（Cloud 返回 `202 queued`），提取在 EverOS 后台进行。

请求超时 6 秒（连接 3 秒）。搜索、写入、注入失败只打日志，**呼叫照常进行**。没有 key、设置里关掉、或 `EVEROS_ENABLED=0` 时客户端不发任何请求。

核心是 Rust，所以直接调 HTTP，不调用 Python SDK `everos-cloud`（那是同一套 v2 路由的薄封装，桌面应用不必再带一个 Python 运行时）。

**启用**

1. 在 [EverOS Console](https://everos.evermind.ai/api-keys) 创建 key。
2. 让**启动 AgentDock 的那个进程**看得到环境变量（从访达打开的 `.app` 读不到你在终端里的 `export`）：

```bash
export EVEROS_API_KEY="你的 key"          # 必填，不会写入仓库或 settings.json
# export EVEROS_BASE_URL="http://127.0.0.1:8000"   # 自建 EverOS OSS，可选
# export EVEROS_USER_ID="user_001"         # 可选；不设则首次开启时生成 ad-<uuid> 并写入 settings.json
# export EVEROS_APP_ID="agentdock"         # 可选
# export EVEROS_PROJECT_ID="default"       # 可选
# export EVEROS_ENABLED=1                  # 可选：不打开设置开关也启用；=0 强制关闭
npm run tauri dev
```

图形会话里要长期生效（macOS）：`launchctl setenv EVEROS_API_KEY "你的 key"`，然后从访达重新打开。自建服务默认不校验 token，但仍需设一个非空的 `EVEROS_API_KEY`，否则 AgentDock 视为未配置。

3. **设置 › 记忆 › 启用长期记忆**。仅有 key、开关关闭时不会上传。
4. 从 Dock 呼叫一位网页席位（设置里的「测试呼叫」只验证选择器，不写记忆）。几秒后到 [EverOS Console](https://everos.evermind.ai) 能看到这条通话；下一通呼叫前，输入框里会出现上次提炼出的记忆。

网页席位的输入框和字幕选择器可以写进驱动 JSON（更高 `version` 放进用户目录即热更新）：

```json
"memory": {
  "composer": ["#prompt-textarea"],
  "transcript": ["[data-message-author-role]"]
}
```

留空则用通用选择器。桌面 App 席位（Grok Bot）没有可注入的输入框，仍然会记录通话起止。

## 测试

```bash
cd src-tauri && cargo test                             # 56 个单元测试：状态机（含切换先挂断）/ Agent / 设置兼容 / 放置 / 形变弹簧 / 快捷键 / 驱动（AX 选择器、Bot 列表、群聊过滤、Chrome）/ EverOS
npm run build                                          # 前端类型检查 + 打包
npx vite --port 1420 & python3 tests/e2e_mock.py       # 31 项浏览器 E2E：状态机→DOM、横竖、拖拽、把手、自动收起、三档收起、设置页（含记忆开关）
bash tests/e2e_xvfb.sh                                 # Linux 真实二进制冒烟（Xvfb）
DOCS_RS=1 cargo check --target aarch64-apple-darwin    # 在 Linux 上交叉检查 macOS objc2 代码
python3 .shots/shoot.py                                # 重新生成 docs/screenshots 下的 mock 截图
```

**持续集成与发版**（[`.github/workflows`](.github/workflows)）

- `CI`：每次推送到 `main` 和每个 PR，在 macOS 上跑前端构建和 Rust 单元测试。
- `Release`：每次推送到 `main` 自动检查 `package.json` 的版本号，还没发布过就打包 macOS 通用版 dmg，发布 `v<版本>` 到 GitHub Releases，「本次更新」列出上个版本以来的提交；版本号没变的推送几秒内就跳过。版本号只在 `package.json` 里改，`tauri.conf.json` 直接读它，所以发新版只要：

```bash
npm version patch          # 0.2.0 → 0.2.1（minor / major 同理；带 -beta.1 这类后缀的发成预发布）
git push --follow-tags     # 推送后流水线自己打包、发布，不用再手动打标签
```

在 Actions 页面手动运行 `Release` 只打包，安装包作为流水线产物下载，不发 Release。

真机验证记录（macOS 26.6.2 · Apple Silicon）：原生 `NSGlassEffectView` 材质、主题切换无残影、拖拽零漂移、重启位置保留、横竖切换、把手、自动收起与三档收起、⌥O / ⌥P / ⌥Space、原生右键菜单。

## 隐私与边界

- **不接触密码。** 登录发生在真实站点页面，由你完成。用 Chrome 时 Cookie 留在 Chrome 里；用内置浏览器时留在 AgentDock 的数据目录里（macOS 上各 Agent 共用一份）。
- **默认不上传。** 没有遥测。只有在「记忆」里开启并且进程里有 `EVEROS_API_KEY` 时，通话事件和页面上可见的对白才会发到 EverOS（Cloud 或你自己的 `EVEROS_BASE_URL`）。
- **界面自动化的天然边界。** 四家都没有公开「呼叫我的 agent」接口，全部走界面自动化；界面改版需要更新驱动 JSON。
- **用 Chrome 时**，AgentDock 通过 Chrome 自己的远程调试连接（需你在 `chrome://inspect` 开启并在弹窗里允许）只操作它为各 Agent 打开的标签页，不读取 Chrome 的 Cookie 文件。
- **Grok Bot 的 Bot 列表**读自它本机的缓存文件（`~/Library/Application Support/Grok Bot/sand-client-persistence` 里的 roster），只读 Bot 名字、是否群聊、是否隐藏，不读聊天内容，也不上传。
- **待实测项。** 隐藏 webview / 后台标签页的 `getUserMedia` 授权与不可见时的 WebRTC 节流需在你登录后的真机上验证。
- **系统级闭麦**在没有音频输入设备的机器上会跳过（只切换 UI 状态）。

## 路线图

- [x] Dock 三态 · 呼叫 / 闭麦 / 挂断 / 切换 / 撤销
- [x] macOS 26 原生 Liquid Glass · 浅深色 · 跟随系统
- [x] 任意位置拖拽 · 横向 / 竖向 · ⇅/⇄ 把手
- [x] 自动收起 · 三档收起方式 · hover 展开
- [x] 全局快捷键（可录制）· 原生右键菜单
- [x] 设置窗：席位拖拽排序 · 自定义图标 / GIF · 快捷键 · 外观 · 麦克风 · EverOS 记忆
- [x] 网页版 Agent 沿用 Chrome 登录（远程调试），内置 webview 回退
- [x] 桌面 App 后台驱动（Grok Bot、ChatGPT）· Grok Bot 每个 Bot 一个联系人 · 群聊过滤 · 切换先挂断
- [x] 程序坞图标打开设置 · 可选仅菜单栏 · 本机固定签名打包
- [ ] 各站选择器真机钉定并标记 `verified: true`（Grok Bot 已真机接通）
- [ ] Windows 实机编译与 Mica 验证
- [ ] 通话中来电提示（另一席位有消息时胶囊轻微呼吸）
- [x] GitHub Actions：CI + 推送新版本号即自动发布 macOS 通用版 dmg
- [ ] Apple 开发者证书签名与公证（免去首次「仍要打开」）

## 致谢

视觉以 Apple Human Interface Guidelines 与 macOS Tahoe 系统控件为准（参考图与还原规格见 [`docs/ref`](docs/ref)）。基于 [Tauri 2](https://tauri.app)、[window-vibrancy](https://github.com/tauri-apps/window-vibrancy)、[objc2](https://github.com/madsmtm/objc2)。

## 参与贡献

欢迎 Issue 和 PR，最常见的是各家界面改版后更新驱动 JSON。开发、测试与提交约定见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 许可证

[MIT](LICENSE) © 2026 we1jia。可以自由使用、修改、分发和商用，保留版权与许可声明即可。各家产品名称与图标归其所有者，仅用于标识对应服务。
