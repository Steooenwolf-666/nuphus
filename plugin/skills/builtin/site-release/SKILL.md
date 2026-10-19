---
title: 官网发布部署规范（www.nuphus.com / dl.nuphus.com）
id: site-release
type: skill
tags: [release, deploy, website, caddy, download, version]
---

# 官网发布部署规范

> 发新版本 / 改官网时的固定流程。基础设施已固化，按清单走，禁止每次重新探索。

---

## 一、基础设施事实（2026-10-19 固化）

### 服务器

| 项 | 值 |
|---|---|
| 官网+下载+中继机 | `43.161.235.81`（腾讯云海外，Ubuntu 24.x，用户 `ubuntu`，2C1G/40G/200Mbps） |
| VPN 机（不碰） | `43.132.220.206` |
| relay-server | `/opt/relay-server/`（勿动） |
| 安装包目录 | `/opt/downloads/<版本号>/` |
| 官网目录 | `/var/www/nuphus-site/`（git 仓库，`sudo git pull` 更新） |
| 反向代理 | Caddy，`/etc/caddy/Caddyfile`（TLS 自动签发，改完必须 `validate` + `reload`） |

### 域名（全部 → 43.161.235.81，NS 在 DNSPod）

| 子域 | 用途 | Caddy 上游 |
|---|---|---|
| `www.nuphus.com` | 官网静态站 | `file_server /var/www/nuphus-site` |
| `nuphus.com` | 裸域 301 → www | `redir` |
| `dl.nuphus.com` | 安装包下载（支持断点续传） | `file_server /opt/downloads` |
| `relay.nuphus.com` | 设备通道 wss | `127.0.0.1:18080` |
| `r.nuphus.com` | 手机隧道入口 | `127.0.0.1:18081` |

### SSH 约束（重要）

本机安全规则**禁止修改文件 ACL**，SSH 私钥权限修不了 → **无法从本机直连服务器**。
标准替代通道：**GitHub 私有仓 `mrpulor-gh/nuphus-site` 中转**（文件变更）+ **OrcaTerm 逐条命令**（服务器侧操作）。
本机 GitHub token 在 git credential manager（`mrpulor-gh`），服务器端 credential.helper store 已存。

### 本地路径

- 官网源文件（编辑处）：`projects/nuphus-site/site/`（被主仓 .gitignore 忽略）
- git 工作副本（推送处）：`projects/nuphus-site/_ghdeploy/`（内容 = site/，改完必须同步）

---

## 二、发版清单（每次发版按序执行）

### 1. 服务器：上传安装包

三平台包传到 `/opt/downloads/<版本号>/`。来源二选一：
- 服务器直连 GitHub（服务器在境外，实测直连可用）：`sudo curl -fsSL -o <文件名> https://github.com/mrpulor-gh/nuphus/releases/download/v<版本>/<文件名>`
- 本机已有：`projects/nuphus-site/` 之外的 `npm-desktop/downloads/<版本>/`

文件名模式：`nuphus-win32-x64-<版本>-setup.exe` / `nuphus-osx-arm64-<版本>.zip` / `nuphus-linux-x64-<版本>.tar.gz`

### 2. 官网：版本号引用同步（易漏！全站共 7 处）

| 文件 | 位置 | 内容 |
|---|---|---|
| `site/download.html` | `<title>` | `下载 · Nuphus <版本>` |
| `site/download.html` | `<meta description>` | 版本号 1 处 |
| `site/download.html` | `.page-head p` | `<版本> 发布于 <日期>` |
| `site/download.html` | `#dl-title` | `Nuphus <版本>` |
| `site/download.html` | `data-file` / 表格文件名 ×3 + 镜像区路径 | `nuphus-*-<版本>-*` |
| `site/download.html` | 全部 `dl.nuphus.com/0.2.30/` 链接 | 改成新版本目录 |
| `site/index.html` | `.hero-ver` | `v<版本>`（hero-meta 三元信息中间位） |

**做法**：全局搜旧版本号字符串（如 `0.2.30`）逐处替换，禁止凭记忆改。改完 `grep -r "0.2.30"` 应零残留。

### 3. 官网：下载链接批量替换

所有 `data-dl-primary` CTA + `os-list` 三行 + `download.html` 平台表，共约 18 处 href，全部指向
`https://dl.nuphus.com/<版本>/<文件名>`。版本号替换（第 2 步）时顺带覆盖。

### 4. 推送 GitHub

```powershell
# 本机：同步 site/ → _ghdeploy/ → push
Remove-Item projects\nuphus-site\_ghdeploy\* -Recurse -Force -Exclude .git
Copy-Item projects\nuphus-site\site\* projects\nuphus-site\_ghdeploy\ -Recurse -Force
git -C projects\nuphus-site\_ghdeploy add -A
git -C projects\nuphus-site\_ghdeploy -c user.name='nuphus' -c user.email='admin@nuphus.local' commit -m 'site: release <版本>'
git -C projects\nuphus-site\_ghdeploy push
```

### 4.5 新增贡献者头像（本地化铁律）

**禁止**在 HTML 里引用 `avatars.githubusercontent.com`——国内访问该域 1.7s+/个且不稳定，首页曾因此拖慢（实测 7 头像 + 轮播重复引用共 14 处）。

新增贡献者时的固定动作：

```powershell
# 1. 下载头像到本地（112px 供 2x 屏，显示 28px）
Invoke-WebRequest -Uri 'https://avatars.githubusercontent.com/u/<UID>?s=112&v=4' -OutFile 'projects\nuphus-site\site\assets\avatars\<用户名>.png'

# 2. HTML 引用本地路径（轮播两组都要改）
<img src="assets/avatars/<用户名>.png" alt="<用户名> 的头像" width="28" height="28">
```

验证：`grep -c 'avatars.githubusercontent.com' site/index.html` 必须为 **0**。
头像文件与官网一同 push/pull，见第 4/5 步。

现有头像：`site/assets/avatars/`（7 位贡献者，fouyzjl / yuansui486 / jiangdingwei123-afk / zhoupeiyu515-ui / Steooenwolf-666 / yyyyyyyyiiiii / mrpulor-gh）。

### 4.6 下载链接必须跟随访客系统（铁律）

`site.js` 平台识别会同时改按钮**文案与 href**：Windows→setup.exe / macOS→zip / Linux→tar.gz。
版本号从页面已有链接动态解析（正则 `/(\d+\.\d+\.\d+)\//`）——**发版只改 HTML 里的版本引用，JS 自动跟随，禁止在 JS 里再写一份版本号**。

配套要求（download.html 主卡）：
- `data-sha-hex`（SHA-256 展示）与 `data-sha-copy`（复制按钮）由内联脚本按系统切换
- 三个平台的 SHA-256 **必须从本地安装包实测计算**（`Get-FileHash`），禁止从旧版复制
- 平台表每行带 `.sha-row` 显示对应哈希

验证：`grep -c 'avatars.githubusercontent.com'` 为 0；24 个 dl 链接正则校验全过；`node --check site.js` 语法过。

### 5. 服务器：拉取生效（OrcaTerm 一条命令）

```bash
cd /var/www/nuphus-site && sudo git pull && echo DONE
```

### 6. 验证（本机，全部必做）

```powershell
# a. 三平台安装包可达 + 断点续传
Invoke-WebRequest -Uri "https://dl.nuphus.com/<版本>/nuphus-win32-x64-<版本>-setup.exe" -Method Head
# 期待 HTTP 200 + Accept-Ranges: bytes

# b. 官网版本号已更新
(Invoke-WebRequest -Uri 'https://www.nuphus.com/download.html' -TimeoutSec 25).Content -match '<新版本号>'
# 期待 True

# c. 裸域重定向
Invoke-WebRequest -Uri 'https://nuphus.com/' -Method Head -MaximumRedirection 0 -ErrorAction SilentlyContinue
# 期待 301 → https://www.nuphus.com/
```

---

## 三、自动更新链路（走 dl.nuphus.com 优先，2026-10-19 起）

`tauri.conf.json` endpoints 四级降级（dl 放首位，原三级保留兜底）：

```
1. https://dl.nuphus.com/latest.json          ← 自建（国内外均可达）
2. https://gitee.com/nuphus/nuphus/raw/main/latest.json
3. https://gh-proxy.com/https://github.com/.../latest.json
4. https://github.com/mrpulor-gh/nuphus/releases/latest/download/latest.json
```

**latest.json 同步职责（发版清单新增第 1.5 步）**：CI 生成的 latest.json（在 GitHub Release 资产里）必须同步到服务器 `/opt/downloads/latest.json`——注意 latest.json **内部的 url 指向 gh-proxy**（release.yml 生成逻辑），updater 拿到后会从镜像下载安装包。若想让更新也走 dl 域名，需在同步时把 url 域名替换为 `https://dl.nuphus.com/<版本>/<文件名>`——**文件名必须与 updater 期望一致**：

| 平台 | updater 拉的文件 | 与手动下载的关系 |
|---|---|---|
| Windows | `nuphus-win32-x64-<v>-setup.exe` | 同一文件（dl 已有）|
| macOS | `Nuphus.app.tar.gz` | **不同文件**（手动是 osx-arm64.zip），必须另传 |
| Linux | `Nuphus_<v>_amd64.AppImage` | **不同文件**（手动是 linux-x64.tar.gz），必须另传 |

**禁止**只同步 latest.json 不传产物——macOS/Linux updater 会 404。

**下载统计**：Caddy 访问日志（JSON）→ systemd timer 每 5 分钟聚合 → `/opt/downloads/stats.json`（total + 按文件计数，不含 IP，无隐私面）。日志滚动 10MiB×5 份防膨胀。

**Gitee 事实**：只有 Windows setup.exe 是真实安装包（zip/tar.gz 是源码归档），macOS/Linux 只能靠官网直连或 GitHub。

---

## 四、坑（已付过学费）

1. **Caddyfile 站点块重复** → `ambiguous site definition`。追加前先 `grep -n '<域名>' /etc/caddy/Caddyfile` 确认不存在。
2. **heredoc 失败半截写入** → 追加型 `tee -a` 在前面命令失败时仍会执行，撞重复。追加了就 `grep` 验证，重复用 `sed -i '<起>,<止>d'` 删。
3. **sudo git clone 属主 root** → 客户端 `dubious ownership`，`git config --global --add safe.directory /var/www/nuphus-site` 解决。
4. **版本号漏改** → 官网 7 处 + 链接 18 处，靠 grep 兜底不靠记忆。
5. **改 Caddyfile 三步走**：改 → `sudo caddy validate --config /etc/caddy/Caddyfile`（必须 Valid）→ `sudo systemctl reload caddy`。跳过 validate 直接 reload，配置坏了服务就挂。

---

## 五、快速参考：当前线上版本

| 项 | 值 |
|---|---|
| 线上版本 | 0.2.30（2026-10-17 发布） |
| 官网 commit | `de94564`（首次上线） |
| 包目录 | `/opt/downloads/0.2.30/`（3 文件，165MB） |
| GitHub 私有仓 | `mrpulor-gh/nuphus-site`（main） |
