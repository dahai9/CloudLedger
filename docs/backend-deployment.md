# CloudLedger 生产部署手册

本文是 CloudLedger v0.1.5 的生产主机操作手册。它描述当前唯一受支持的
部署方式：systemd Linux + Docker Compose + GHCR 镜像 + Cloudflare Origin CA
以及 rclone crypt/OneDrive。管理员只需要运行一个脚本并使用数字菜单：

```bash
sudo ./deploy/cloudledger-ops.sh
```

脚本不接受日常公开子命令。`--internal backup`、`--internal health`、
`--internal restore-test` 和 `--internal firewall-refresh` 仅供已安装的
systemd service 调用。不要把它们当作人工操作入口。

> 版本边界：本文以仓库 tag `v0.1.5` 和脚本顶部显示的 `v0.1.5` 为准。
> 生产镜像必须使用明确 tag，`latest` 会被拒绝。若脚本版本、Compose 文件和
> 本文不一致，先停止部署并统一到同一个 tag。

## 1. 先选择部署路线

| 场景 | 菜单路径 | 说明 |
| --- | --- | --- |
| 全新 VPS | `1 → 11` | 执行完整安装向导，最后才启用备份和恢复演练定时器 |
| 已运行当前格式 | `3 → 3` | 选择目标 tag；升级前必须先有可下载、可回验的 crypt 备份 |
| 已运行已知 v0.1.3 布局 | `3 → 3` | 仅当旧 Compose、Caddy、端口和镜像都精确匹配时才会进入接管流程 |
| 日常查看/操作 | 主菜单 `2` 至 `11` | 每一级 `0` 返回；无须记忆 Docker 服务名或文件路径 |

不要在同一台主机上同时使用本手册的 Compose 部署、手工启动的后端二进制、
宿主 PostgreSQL 或宿主 Caddy。它们会争用端口、数据库和配置，且不受运维工具的
备份与回滚保护。

## 2. 部署后的网络和容器结构

```text
公网客户端
    │ HTTPS（Cloudflare 橙云）
    ▼
VPS:443 ── nftables 仅允许 Cloudflare 官方 IPv4/IPv6 ──┐
                                                         ▼
                                                network-anchor
                                                         │ 共享 network namespace
                           ┌─────────────────────────────┼────────────────────┐
                           ▼                             ▼                    ▼
                       Caddy                    CloudLedger 后端         PostgreSQL
                  127.0.0.1:8787               127.0.0.1:8787          127.0.0.1:5432

管理员电脑
    │ ssh -N -L 8788:127.0.0.1:8788
    ▼
VPS 127.0.0.1:8788 → network-anchor:18788 → 后端 127.0.0.1:8788
```

五个 Compose 服务共享 `network-anchor` 的网络命名空间：

- `network-anchor`：唯一声明主机端口和管理端 relay。
- `cloudledger`：使用 `cloudledger_runtime` 最小权限数据库账号。
- `postgres`：只在共享命名空间的回环地址监听。
- `migration`：Compose `migration` profile 的一次性迁移服务。
- `caddy`：使用 Origin CA 证书终止 HTTPS，代理到后端回环地址。

固定端口不可改成公网管理端口：

| 主机监听 | 用途 | 暴露范围 |
| --- | --- | --- |
| `127.0.0.1:18080` | Caddy/HTTP 本机诊断入口 | 仅 VPS 本机 |
| `443` | Cloudflare HTTPS 源站 | nftables 只接受 Cloudflare IP |
| `127.0.0.1:8788` | 管理端 SSH 隧道入口（映射容器 `18788`） | 仅 VPS 回环地址 |
| `80` | CloudLedger 不占用 | 其他既有服务使用 |

如果 `443` 已被其他进程或容器占用，向导会在修改防火墙或停止服务前失败。不要
通过关闭防火墙、把管理端绑定到 `0.0.0.0` 或抢占端口来绕过检查。

## 3. 开始前准备

### 3.1 VPS 和网络

准备一台可通过 SSH 登录并拥有 `sudo` 的 64 位 Linux 主机。必须满足：

- PID 1 为 systemd；支持 Debian、Ubuntu、RHEL、Rocky Linux 及兼容发行版。
- 可出站访问 Docker 官方源、GHCR、GitHub API、Cloudflare HTTPS 和 OneDrive。
- 主机时间正确，磁盘有数据库、Docker 卷和至少一份本地备份的空间。
- SSH 用户能够在部署期间执行 root 操作；不要在生产中使用
  `CLOUDLEDGER_ALLOW_NONROOT=1`。

不支持 Docker Desktop、仅 OpenRC 的系统和非 systemd 主机。脚本会检查
`docker`、Compose plugin、`curl`、`jq`、`openssl`、`tar`、`sha256sum`、
`flock`、`rclone`、`nft`、`ss`、`systemctl`；缺少的依赖可由菜单自动安装。

### 3.2 Cloudflare

在 Cloudflare 控制台先完成以下内容：

1. 为 API 创建 A/AAAA DNS 记录，指向 VPS，打开橙色云（Proxied）。
2. SSL/TLS 加密模式设为 **Full (strict)**。
3. 创建 Origin CA 证书，证书 SAN 必须覆盖 API 域名（通配符也必须实际匹配）。
   下载 PEM 证书和未加密 PEM 私钥，私钥不要粘贴到聊天或工单中。
4. 创建生产 Turnstile widget，记录 site key 和 secret key。widget 的
   Hostname 必须覆盖实际使用的浏览器页面；Tauri/loopback 页面不一定因为
   配置了 API 域名而自动被覆盖。出现 `invalid domain` 时停止验收并回到
   Turnstile 控制台修正 hostname。
5. 记录 Cloudflare zone 的代理和源站访问变更流程。脚本会从官方
   `www.cloudflare.com/ips-v4` 和 `ips-v6` 获取源站允许列表。

管理端没有 Cloudflare 公网 DNS 路由，也不需要为管理端创建 DNS 记录。它只通过
   SSH 隧道访问。

### 3.3 GHCR 镜像

确认目标 tag 已在 GHCR 发布四个同版本镜像：

```text
ghcr.io/<owner>/cloudledger-server:<tag>
ghcr.io/<owner>/cloudledger-postgres:<tag>
ghcr.io/<owner>/cloudledger-caddy:<tag>
ghcr.io/<owner>/cloudledger-network-anchor:<tag>
```

公开包可匿名拉取；私有包准备具有 `read:packages` 的 GitHub PAT。PAT 只在
菜单隐藏输入中使用，不会写入 `/etc/cloudledger/ops.env` 或备份包。

### 3.4 OneDrive 和 rclone crypt

生产安装需要一个 rclone `crypt` remote，推荐命名如下：

```text
OneDrive remote: onedrive
crypt remote:   cloudledger-crypt
crypt 底层路径: onedrive:CloudLedger
备份目录:       cloudledger-crypt:backups
```

名称可以不同，但最终填写的值必须是 `cryptRemote:path`，且脚本会检查 remote
类型确实为 `crypt`。在服务器外保存以下恢复材料；它们不会进入备份包：

- OneDrive remote 的 OAuth 配置和 token 恢复方式。
- crypt 密码以及 crypt 的 `password2`/salt。
- 当前 GHCR owner、发布 tag、API 域名和本文版本。

不要把 OAuth token、crypt 密码、PAT、数据库密码、管理员 token 或 webhook token
发到聊天中。

## 4. 获取并启动工具箱

脚本必须和 `deploy/docker-compose.yml`、`deploy/Caddyfile`、
`deploy/postgres_roles.sql`、`deploy/systemd/` 位于同一个仓库 checkout 中。建议
在 VPS 上使用明确 tag 的浅克隆（仓库地址按实际 fork 调整）：

```bash
git clone --branch v0.1.5 --depth 1 https://github.com/dahai9/CloudLedger.git \
  /tmp/cloudledger-v0.1.5
cd /tmp/cloudledger-v0.1.5
git describe --tags --exact-match
sudo ./deploy/cloudledger-ops.sh
```

如果已有受信任 checkout，确认 `git status --short` 没有未审查改动，再从该目录
运行脚本。脚本启动时会显示版本、服务、数据库、HTTPS、最近备份和磁盘使用率。
首次向导完成并把资源安装到 `/opt/cloudledger` 后，日常操作应使用该目录中已安装的
同一版本脚本：

```bash
sudo /opt/cloudledger/cloudledger-ops.sh
```

不要继续使用已经删除、改动或版本不明的 `/tmp` checkout。

非 TTY 或需要禁用颜色时可以设置 `NO_COLOR=1`，但仍然必须通过数字菜单操作：

```bash
sudo env NO_COLOR=1 ./deploy/cloudledger-ops.sh
```

## 5. 全新 VPS：完整安装向导

进入 `1. 首次安装与部署`，选择 `11. 执行全部安装向导`。向导失败会停在失败步骤，
不要继续下一菜单硬启动服务。它按以下顺序执行：

### 第一步：暂存资源和检查依赖

工具把经过当前 checkout 的 Compose、Caddyfile、角色 SQL、脚本、旧版模板和
systemd 文件原子复制到 `/opt/cloudledger`。随后检查 systemd、Docker、Compose 和
辅助工具。

缺依赖时脚本会根据发行版使用 apt/dnf 和 Docker 官方源安装。已有 Docker 时，安装
Compose plugin 前会显示正在运行的容器并要求确认；不会主动重启已有 Docker daemon。
安装过程需要 root 和可用的软件源。

### 第二步：GHCR 登录和固定 tag

向导询问公开或私有 GHCR。私有仓库在隐藏输入框粘贴 PAT；完成拉取后不要把 PAT
写进脚本、shell history 或备份。

输入仓库 owner 和明确 tag，例如 `v0.1.5`。四个镜像会被写入
`/etc/cloudledger/ops.env`，脚本会拒绝 `latest`、不匹配的镜像名、不同 owner 或
不同 tag。

### 第三步：生成密钥

工具使用 OpenSSL 生成三个数据库密码、随机管理路径、管理 token、审计 key id、
审计 HMAC 和 identifier HMAC。它们只写入权限为 `0600` 的 `ops.env`；如果检测到
已有密钥，选择保留，除非你已经计划执行数据库角色密码轮换。

角色用途必须保持分离：

| 账号 | 用途 | 后端是否使用 |
| --- | --- | --- |
| `cloudledger_bootstrap` | 运维工具、建库和账号加固 | 否 |
| `cloudledger_migration` | 专用迁移 profile | 否（一次性） |
| `cloudledger_runtime` | API 和管理端运行时 | 是，且为最小权限 |

`cloudledger_bootstrap` 不是前台业务账号；不要把它的连接 URL 放进
`server.toml`。

### 第四步：域名、Turnstile 和 Origin CA

向导只要求一个 API 域名，并固定写入：

```text
CLOUDLEDGER_HTTP_PUBLISH=127.0.0.1:18080:80
CLOUDLEDGER_HTTPS_PUBLISH=443:443
管理端=127.0.0.1:8788（Compose 中映射为 18788 relay）
```

输入 Turnstile site key；secret key 使用隐藏输入。然后输入 Origin CA 证书和私钥
在本机上的路径。导入事务会检查证书可解析、私钥无口令、证书与私钥匹配、SAN
覆盖 API 域名且剩余有效期至少 30 天；失败会恢复导入前的证书。

### 第五步：生成后端配置和配置 rclone

工具生成 `/etc/cloudledger/server.toml`：

```text
mode = reverse_proxy
API 绑定 127.0.0.1:8787
管理端绑定 127.0.0.1:8788
PostgreSQL 使用 cloudledger_runtime
database.auto_migrate = false
```

配置 rclone 时选择“打开 rclone 数字配置向导”。OneDrive OAuth 通常需要浏览器
回调；先在管理员电脑建立隧道：

```bash
ssh -N -L 53682:127.0.0.1:53682 -p <SSH端口> <SSH用户>@<VPS地址>
```

保持该窗口运行，在另一个 SSH 会话中运行安装向导；当 rclone 要求浏览器回调时，
在本机浏览器完成授权。完成后在 `7. OneDrive / rclone 管理` 中选择：

```text
7 → 5  测试远程连接
7 → 6  测试上传和下载
7 → 9  设置 crypt 远程备份目录（例如 cloudledger-crypt:backups）
```

配置文件固定为 `/etc/cloudledger/rclone.conf`，权限 `0600`。上传/下载/内容比对
不通过时，向导不会开始首次部署。

### 第六步：部署、迁移和网络保护

向导执行以下不可跳过的流水线：

1. 预检主机 `443` 的占用者。
2. 拉取四个同 tag GHCR 镜像。
3. 启动 `network-anchor` 和 PostgreSQL，等待数据库健康。
4. 拉取 Cloudflare IPv4/IPv6，原子应用 nftables 的 Cloudflare-only 443 规则。
5. 验证三个 PostgreSQL 角色及其迁移元数据权限。
6. 通过 `migration` profile 执行 SQLx 迁移并验证审计链。
7. 启动后端，检查本地 `/health` 和 `/ready`。
8. `caddy validate` 通过后启动 Caddy。
9. 通过 Cloudflare 检查公网 `/health`、`/ready` 和 Turnstile secret。
10. 复核防火墙状态。

任何一步失败都应保留现场并查看日志，不要直接 `docker compose up -d` 绕过顺序。

### 第七步：安装定时任务、备份和恢复演练

部署成功后安装 8 个 systemd 单元（4 service + 4 timer），先启用健康检查和
Cloudflare 防火墙刷新。然后创建第一份完整加密备份，执行一次真实的临时数据库恢复
演练，全部通过后才启用每日备份和每周恢复演练 timer。向导最后再次执行完整验收。

## 6. 从 v0.1.3 受控接管到 v0.1.5

这是针对已知旧生产布局的窄路径，不是任意旧 Compose 的通用升级。开始前必须满足：

- server 和 PostgreSQL 镜像都是 `v0.1.3`，且来自当前信任的同一 GHCR owner。
- 旧 Compose 与 `deploy/legacy/compose-v0.1.3.yml` 完全一致；旧 Caddyfile 是当前
  信任模板。
- 旧端口是 `18080`、`443`、`8788`，Origin CA 证书仍覆盖 API 域名且剩余至少 30 天。
- 旧 `server.toml` 能唯一解析出管理路径/token、审计密钥和 Turnstile，并与
  `ops.env` 的 Turnstile 值一致。
- `/etc/cloudledger/rclone.conf` 已配置 crypt remote，且 `7 → 5`、`7 → 6` 均通过。
- 先完成一份可下载和校验的完整备份；没有远程 crypt 备份时升级会在修改前停止。

操作步骤：

1. 运行 `sudo ./deploy/cloudledger-ops.sh`。
2. 进入 `3. 版本升级与数据库迁移`，选择 `3. 升级到指定版本`。
3. 输入明确目标 tag `v0.1.5`，阅读影响范围并输入 `YES`。
4. 等待工具完成镜像 manifest 检查、旧配置快照、备份上传/下载回验、配置规范化、
   迁移、健康检查、Caddy 和防火墙复核。

迁移开始前发生失败或收到中断信号时，工具会恢复 `ops.env`、Compose、Caddyfile、
已安装脚本、角色 SQL、`server.toml` 和固定 Origin CA 文件，并尝试启动旧服务。
迁移开始后绝不自动降级；只能从与数据库、配置和审计密钥匹配的备份恢复。升级记录
在 `3 → 7`，失败现场在 `3 → 9`。

## 7. 部署完成后的验收

不要把脚本返回“命令执行成功”当作公网部署完成。依次完成以下菜单检查，并保存结果
和时间：

| 验收内容 | 菜单 |
| --- | --- |
| 四服务运行状态 | `2 → 1` |
| 当前四个镜像 tag | `2 → 9` 或 `3 → 1` |
| 迁移记录和目标版本 | `3 → 4` |
| 审计链 | `3 → 6` |
| 完整健康、证书和防火墙 | `5 → 11` |
| Origin CA 信息/SAN/有效期 | `6 → 5/6/7` |
| Caddyfile 语法和重载 | `6 → 8/9` |
| Cloudflare 代理 `/health` | `6 → 10` |
| rclone 连接、传输、备份目录 | `7 → 5/6/8` |
| 敏感文件权限和数据库角色 | `8 → 7/9` |
| timer 状态及下次时间 | `10 → 1/11` |

完整健康检查应看到：后端和 PostgreSQL 健康、`/health` 和 `/ready` 成功、Turnstile
secret 探针符合预期、审计链成功、Origin CA 未过期、防火墙包含有效的 Cloudflare
IPv4/IPv6 集合。证书或 Cloudflare 任一检查失败都不能对外宣称部署完成。

### 管理端 SSH 隧道

在管理员电脑保持隧道：

```bash
ssh -N -L 8788:127.0.0.1:8788 -p <SSH端口> <SSH用户>@<VPS地址>
```

随后在本机浏览器访问 `http://127.0.0.1:8788/<随机管理路径>`。随机路径位于
VPS 的 `/etc/cloudledger/server.toml` `[admin]` 段；应通过 root-only 的本地受控
方式读取（例如只打印 `path` 键，不打印整个文件）：

```bash
sudo awk '
  /^\[admin\]/{in_admin=1; next}
  /^\[/{in_admin=0}
  in_admin && /^[[:space:]]*path[[:space:]]*=/ {print}
' /etc/cloudledger/server.toml
```

把路径保存在密码管理器中，绝不要记录管理 token。固定 `/admin` 必须返回 404，
管理端不能在公网 DNS、Caddy 或 Cloudflare 上出现。

### 客户端 API 地址

发布 Android/Web 客户端前，检查 `frontend/public/config.js`。当前 checkout 中如
仍是旧的 ngrok 地址，必须在发布构建前改成生产 API：

```js
window.__CLOUDLEDGER_CONFIG__ = {
  apiBaseUrl: "https://<API域名>",
};
```

这只是客户端发布配置，不要把数据库密码、Turnstile secret 或管理员 token 放入
`config.js`。本手册不会自动修改该文件。

## 8. 日常服务和版本管理

### 服务管理（主菜单 `2`）

`2 → 1` 查看全部服务；`2 → 2/3/4` 启动、停止、重启全部服务；`2 → 5/6/7`
先选择 `1 CloudLedger 后端`、`2 PostgreSQL`、`3 Caddy` 或 `4 Network Anchor` 再
操作单个服务；`2 → 8` 看容器详情；`2 → 9` 看镜像；`2 → 10` 拉取当前配置的镜像。
停止/重启全部服务会显示影响范围并要求确认。

### 升级流水线（主菜单 `3`）

`3 → 2` 查询 GitHub Release，`3 → 3` 输入明确 tag。工具固定执行：目标镜像检查 →
配对备份 → 拉取镜像 → 停止入口和旧后端 → migration profile → 迁移/审计验证 → 启动
新后端 → `/health`/`/ready` → Caddy → Cloudflare 和防火墙复核。

`3 → 5` 是人工迁移入口，仅在已确认数据库健康并阅读影响范围后使用；生产服务保持
`database.auto_migrate = false`。`3 → 8` 只显示回滚边界：迁移前由工具自动恢复，迁移
开始后必须用匹配备份恢复。`3 → 10` 是一次性数据库账号加固，工具会先备份，再创建
bootstrap、迁移对象所有权和 runtime/migration 最小权限，失败会回滚 SQL 事务。

## 9. 备份、恢复与灾难恢复

### 9.1 完整备份内容和保证

`4 → 1` 创建完整备份。数据库使用一致性的 `pg_dump -Fc`，归档必须包含且只能包含
以下九个文件：

```text
postgres.dump
server.toml
compose.env
compose.yml
Caddyfile
origin-cert.pem
origin-key.pem
manifest.json
SHA256SUMS
```

明文暂存目录固定为 `0700`，任务结束由 trap 清理。工具先生成隐藏 `.new` 文件，执行
manifest、成员列表、大小、SHA-256、dump 格式、tag/owner、域名、证书 SAN 和当前
模板校验；再上传 crypt 远程的隐藏 `.new` 对象，下载并逐字节比对，最后原子发布正式
文件名。上传、下载或比对失败不会删除旧备份。

本地 `tar` 归档本身不是加密文件，里面含有数据库和 Origin 私钥；它只应留在
`/var/lib/cloudledger-ops/backups` 的 root-only 目录，并纳入主机磁盘加密、访问审计
和离线清理策略。OneDrive 远程对象必须通过 rclone crypt 加密。

默认本地和远程保留最近 30 份。只有新备份已经验证成功才会清理更旧对象；远端清理
失败时任务按失败处理，不要手工删除旧备份来“腾空间”后再假装成功。

### 9.2 常用备份菜单

- `4 → 2` 列出 OneDrive/crypt 远程备份。
- `4 → 3` 查看归档成员；`4 → 4` 校验指定备份。
- `4 → 5` 下载远程备份到受保护本地目录并校验。
- `4 → 6` 恢复指定备份：先显示覆盖范围、确认一次，再完整输入备份文件名。
- `4 → 7` 执行临时数据库恢复演练；优先下载并验证远程最新备份。
- `4 → 8` 查看最近一次备份；`4 → 9` 清理超出保留数的本地旧备份。
- `4 → 10` 修改 systemd `OnCalendar` 和保留数量；`4 → 11` 发送 webhook 失败告警测试。

恢复过程会停止 Caddy 和后端、创建恢复前数据库/配置回滚快照、恢复 dump、安装同
一备份的配置和证书、应用角色密码并重新验证服务。任何步骤失败都会尝试回滚；回滚
快照清理失败时任务仍按失败处理并保留现场。

### 9.3 空 VPS 灾难恢复的边界

当前恢复事务要求已有一个可工作的 CloudLedger 部署，用来创建恢复前快照。因此
“恢复指定备份”不是空 VPS 的单命令恢复。正确流程是：

1. 在新 VPS 上使用与备份兼容的明确 tag 建立隔离的空白 CloudLedger 部署，不切生产 DNS。
2. 配置同一个 OneDrive remote、crypt remote 以及服务器外保存的 crypt 密码/salt。
3. `4 → 5` 下载目标归档，`4 → 4` 校验通过。
4. `4 → 6` 选择归档，二次输入完整文件名确认恢复。
5. 完成服务、角色、迁移记录、审计链、Origin CA、Cloudflare 和 `4 → 7` 恢复演练验收。
6. 验收全部通过后，才把 Cloudflare DNS 切到新 VPS。

crypt 密码不在备份包中；没有它即使拿到归档也无法恢复。远程恢复演练默认拒绝
超过 72 小时的备份，并使用 `/var/lib/cloudledger-ops/last-remote-backup` 防止
远程列表回滚到主机已经见过的更旧对象。

## 10. Cloudflare、证书与防火墙运维

在 `6. Cloudflare 与 HTTPS 证书` 中：

| 操作 | 菜单 |
| --- | --- |
| 查看域名和固定发布端口 | `6 → 1` |
| 修改 API 域名并重绘 server.toml | `6 → 2` |
| 查看 SSH 隧道提示 | `6 → 3` |
| 成对导入 Origin CA | `6 → 4` |
| 查看证书主题、发行者、日期、SAN | `6 → 5` |
| 检查 SAN 覆盖 API 域名 | `6 → 6` |
| 检查剩余有效期（少于 30 天警告） | `6 → 7` |
| `caddy validate` | `6 → 8` |
| 重载 Caddy | `6 → 9` |
| 访问 Cloudflare API `/health` | `6 → 10` |
| 获取并应用 Cloudflare-only nftables | `6 → 11` |

防火墙刷新使用独立 nftables 表和 `input`/`forward` 链，仅收放 443 的 Cloudflare
官方网段，并在应用前运行 `nft --check`。它不管理 SSH、xray、主机 80，也不依赖或
控制 `docker.service`；刷新失败不会停止其他 Docker 容器。直接访问源站的非
Cloudflare 流量必须被拒绝后，Caddy 才能信任 `CF-Connecting-IP`。

## 11. rclone/OneDrive 管理

`7 → 1` 检查安装；`7 → 2` 配置 OneDrive remote；`7 → 3` 配置 crypt；`7 → 4`
查看脱敏配置；`7 → 5` 测试连接；`7 → 6` 测试上传/下载/比对；`7 → 7` 查看空间；
`7 → 8` 列出备份目录；`7 → 9` 修改 `cryptRemote:path`；`7 → 10` 重新运行配置；
`7 → 11` 查看灾难恢复所需材料。

脚本可能调用 rclone 的数字配置向导，因此 OAuth 浏览器回调要用前文的
`ssh -L 53682` 隧道。配置文件权限必须是 `0600`。`crypt` 的密码、salt 和恢复
说明必须存放在服务器外；不要把它们写入 `ops.env`、systemd unit 或备份包。

## 12. 系统配置、安全和权限

`8 → 1` 输出脱敏的 ops/server 配置；`8 → 2` 修改 GHCR owner/tag；`8 → 3` 轮换
数据库密码（会同步角色、重绘 TOML 并重启后端）；`8 → 4` 修改 Turnstile；`8 → 5`
修改 webhook（隐藏输入，空值禁用）；`8 → 6` 修改磁盘 80%/90% 和内存 85% 阈值；
`8 → 7/8` 检查或修复权限；`8 → 9` 验证 PostgreSQL 角色；`8 → 10` 执行一次性账号
加固；`8 → 11` 重绘并规范化部署配置；`8 → 12` 导出脱敏诊断配置。

应保持以下权限：

```text
/etc/cloudledger/ops.env              0600
/etc/cloudledger/server.toml          0600，UID/GID 10001:10001
/etc/cloudledger/rclone.conf          0600
/etc/cloudledger/caddy/origin-key.pem 0600
/etc/cloudledger/caddy/origin-cert.pem 0644
/var/lib/cloudledger-ops              0700（状态、日志、备份、锁）
```

诊断导出不得包含数据库密码、管理员 token、审计 HMAC、Origin 私钥、GHCR PAT 或
rclone 密码。若脱敏自检失败，脚本会删除报告并报告失败；不要手工上传原始配置。

## 13. 监控、日志和定时任务

### 13.1 监控和日志

`5 → 1` 综合面板；`5 → 2` 每 5 秒实时刷新主机、容器、PostgreSQL 和 API；
`5 → 3/4/5/6` 查看 CPU/内存、磁盘/Docker 卷、容器压力、PostgreSQL 连接与库大小；
`5 → 7/8` 检查 `/health`/`/ready`；`5 → 9/10` 检查 Caddy/HTTPS 和证书；`5 → 11`
完整健康检查；`5 → 12` 最近告警。

默认阈值是磁盘 80% 警告、90% 严重，内存 85% 警告，Origin CA 剩余少于 30 天警告。

`9 → 1/2/3/4` 查看后端、PostgreSQL、Caddy 或全部实时日志；`9 → 5` 筛选最近
错误；`9 → 6/7` 查看备份/健康日志；`9 → 8` 完整环境诊断；`9 → 9/10/11` 检查
端口、Docker 网络、DNS/HTTPS；`9 → 12` 导出脱敏报告。

重要状态文件和日志位于 `/var/lib/cloudledger-ops`，但人工只应通过菜单查看。升级
失败快照可能以 `upgrade-failed-old-*` 保留在该目录，必须先复制到受控的离线故障
记录，再按组织的保留政策清理。

### 13.2 systemd 单元和默认计划

向导会安装以下 8 个文件到 `/etc/systemd/system`：

```text
cloudledger-ops-backup.service       cloudledger-ops-backup.timer
cloudledger-ops-health.service       cloudledger-ops-health.timer
cloudledger-ops-restore-test.service cloudledger-ops-restore-test.timer
cloudledger-ops-firewall-refresh.service
cloudledger-ops-firewall-refresh.timer
```

四个 service 的内部入口分别如下；这些参数由 systemd 使用，管理员日常不应手动执行：

| service | `ExecStart` |
| --- | --- |
| `cloudledger-ops-backup.service` | `/opt/cloudledger/cloudledger-ops.sh --internal backup` |
| `cloudledger-ops-health.service` | `/opt/cloudledger/cloudledger-ops.sh --internal health` |
| `cloudledger-ops-restore-test.service` | `/opt/cloudledger/cloudledger-ops.sh --internal restore-test` |
| `cloudledger-ops-firewall-refresh.service` | `/opt/cloudledger/cloudledger-ops.sh --internal firewall-refresh` |

默认计划：每日备份 `03:00`（随机延迟 10 分钟）、健康检查开机 5 分钟后并每 5 分钟、
每周日 `04:00` 恢复演练（随机延迟 15 分钟）、防火墙开机 5 分钟后及每日刷新（随机
延迟 30 分钟）。所有 timer 使用 `Persistent=true`。

`10 → 1` 查看全部 timer，`10 → 2/3` 启用或禁用每日备份，`10 → 4` 修改备份
`OnCalendar`，`10 → 5/6` 启用或禁用健康检查，`10 → 7` 修改健康频率，`10 → 8/9`
启用或禁用恢复演练，`10 → 10` 立即选择并执行一个内部任务，`10 → 11` 查看下次运行
时间。启用每日备份前必须先通过真实远程传输；启用恢复演练前必须先通过一次演练。

备份、恢复、升级和账号加固通过 `flock` 使用同一运维锁，不会并发修改数据库或配置。

## 14. 失败处理和停止条件

遇到下列任一情况，保持服务/数据现场，记录菜单输出和时间，不要盲目重试或降级：

- `443` 被非 CloudLedger 进程占用，或 Cloudflare 网段无法下载/校验。
- 镜像不存在、owner/tag 不一致、使用了 `latest`，或私有 GHCR 权限不足。
- Origin CA 证书与私钥不匹配、SAN 不覆盖 API 域名、有效期不足 30 天或私钥带口令。
- Turnstile `/auth/security` 未启用，或 Cloudflare `siteverify` 把 secret 判定为无效。
- PostgreSQL 不健康、角色权限不符合最小权限、migration profile 或审计链失败。
- rclone remote 不是 `crypt`、OAuth/上传/下载/内容比对失败。
- 备份成员、manifest、SHA-256、pg_dump 格式或模板信任校验失败。
- 迁移已经开始但新后端 `/health`、`/ready`、Caddy 或审计验证失败。

迁移前失败由升级事务自动恢复旧镜像和部署资源；迁移后失败禁止直接把旧镜像重新
启动，必须使用匹配的数据库、`server.toml`、Compose、Caddyfile、Origin CA 和审计
密钥备份恢复。任何自动恢复失败都要保留 `upgrade-failed-old-*` 现场并进行人工审查。

常见错误与处理：

| 现象 | 处理 |
| --- | --- |
| 缺少工具或 systemd | `1 → 1` 检查；确认发行版、软件源和 PID 1 后再 `1 → 2` |
| GHCR pull 失败 | `1 → 3` 重新登录，确认 PAT 具备 `read:packages`，再核对 owner/tag |
| 证书校验失败 | `6 → 4` 重新成对导入，检查 SAN、未加密私钥和有效期 |
| Turnstile invalid domain | 在 widget 中加入真实页面 hostname，重新 `8 → 4` 并验收 |
| rclone crypt 失败 | `7 → 4/5/6` 检查 remote 类型、OAuth、密码和时间；不要改成普通 OneDrive 目录 |
| 数据库迁移失败 | 查看 `9 → 2/5` 和 `3 → 9`；迁移前可自动恢复，迁移后只能匹配备份恢复 |
| 外部 HTTPS 失败 | 先检查 DNS 橙云、Full (strict)、443 防火墙、证书 SAN，再检查 `6 → 8/9/10` |

## 15. 完整数字菜单速查

以下是脚本当前版本的菜单名称；如果运行时显示不同内容，以脚本显示为准并停止
继续对照旧文档操作。

```text
主菜单
1 首次安装与部署       2 服务管理
3 版本升级与数据库迁移  4 数据备份与恢复
5 服务监控与压力查看    6 Cloudflare 与 HTTPS 证书
7 OneDrive / rclone 管理  8 系统配置与安全管理
9 日志与故障诊断        10 定时任务管理
11 关于与环境信息       0 退出

1 首次安装与部署
  1 检查服务器是否满足部署要求
  2 自动安装 Docker、Compose 和辅助工具
  3 配置 GitHub Container Registry
  4 选择要部署的 CloudLedger 版本
  5 生成数据库账号和密码
  6 生成后端 server.toml
  7 导入 Cloudflare Origin CA 证书
  8 配置 API 域名和管理端本地监听（随后配置 Turnstile）
  9 执行完整首次部署
 10 验证首次部署结果
 11 执行全部安装向导
  0 返回

2 服务管理
  1 查看所有服务状态       2 启动全部服务
  3 停止全部服务           4 重启全部服务
  5 启动单个服务           6 停止单个服务
  7 重启单个服务            8 查看容器详细信息
  9 查看当前镜像版本       10 拉取当前配置的镜像
  单服务：1 后端，2 PostgreSQL，3 Caddy，4 Network Anchor，0 返回

3 版本升级与数据库迁移
  1 查看当前版本            2 查询可用 GitHub Release 版本
  3 升级到指定版本           4 检查数据库迁移状态
  5 手动执行数据库迁移       6 验证审计链
  7 查看最近一次升级记录      8 迁移前回滚说明
  9 查看升级失败现场         10 加固已有数据库账号权限

4 数据备份与恢复
  1 立即创建完整备份          2 查看 OneDrive 远程备份
  3 查看某个备份详情          4 校验指定备份
  5 下载指定备份              6 恢复指定备份
  7 临时数据库恢复演练        8 查看最近一次备份结果
  9 清理超过保留数量的备份    10 修改备份时间和保留数量
  11 测试备份失败告警

5 服务监控与压力查看
  1 综合状态面板              2 实时监控（每 5 秒刷新）
  3 CPU/内存                  4 磁盘/Docker 卷
  5 容器资源压力              6 PostgreSQL 连接/数据库大小
  7 API /health               8 API /ready
  9 Caddy/HTTPS               10 Origin CA 有效期
  11 完整健康检查             12 最近健康告警

6 Cloudflare 与 HTTPS 证书
  1 当前域名配置              2 修改 API 域名
  3 管理端 SSH 隧道提示        4 导入 Origin CA 证书和私钥
  5 证书信息                  6 证书覆盖的域名
  7 证书有效期                8 验证 Caddyfile
  9 重载 Caddy                10 检查 Cloudflare 代理访问
  11 刷新并应用 Cloudflare-only 防火墙

7 OneDrive / rclone 管理
  1 检查 rclone               2 创建 OneDrive remote
  3 创建 rclone crypt remote  4 查看当前远程配置（脱敏）
  5 测试远程连接              6 测试上传和下载
  7 远程空间                  8 CloudLedger 备份目录
  9 修改远程目录              10 重新配置 rclone
  11 灾难恢复所需信息

8 系统配置与安全管理
  1 脱敏全部配置              2 修改 GHCR 镜像仓库
  3 修改数据库密码            4 修改 Turnstile
  5 修改 webhook 告警地址     6 修改资源告警阈值
  7 检查文件权限              8 修复文件权限
  9 验证 PostgreSQL 角色      10 加固迁移账号权限
  11 重新生成部署配置         12 导出脱敏诊断配置

9 日志与故障诊断
  1 后端实时日志              2 PostgreSQL 实时日志
  3 Caddy 实时日志             4 全部服务日志
  5 最近错误日志              6 备份任务日志
  7 健康检查日志              8 完整环境诊断
  9 端口占用                  10 Docker 网络
  11 DNS 和 HTTPS              12 导出脱敏诊断报告

10 定时任务管理
  1 全部定时任务               2/3 启用/禁用每日备份
  4 修改每日备份时间           5/6 启用/禁用健康检查
  7 修改健康检查频率           8/9 启用/禁用每周恢复演练
  10 立即执行一次定时任务      11 下次运行时间
  立即任务子菜单：1 每日备份，2 健康检查，3 每周恢复演练，4 防火墙刷新

11 关于与环境信息
  1 关于 CloudLedger           2 查看环境信息
```

每个子菜单都有 `0` 返回上一级；错误数字、空输入和取消都会留在当前菜单或返回，
不会退出整个脚本。危险操作先显示影响范围并二次确认；数据库恢复还要求再次输入
完整备份文件名。

## 16. 最终交付清单

将以下内容作为生产变更的验收记录：

- [ ] VPS 为支持的 systemd Linux，SSH sudo/root 和出站网络已确认。
- [ ] Cloudflare DNS 橙云、Full (strict)、Origin CA SAN 和有效期已确认。
- [ ] Turnstile site/secret 通过 API 状态和 `siteverify`，实际客户端 hostname 已验证。
- [ ] 四个 GHCR 镜像是同 owner、同明确 tag；主机未本地构建镜像。
- [ ] `network-anchor`、后端、PostgreSQL、migration、Caddy 的共享命名空间和固定端口未被改动。
- [ ] `443` 仅允许 Cloudflare IP；80、SSH 和其他服务未被工具接管。
- [ ] `cloudledger_bootstrap`、`cloudledger_migration`、`cloudledger_runtime` 角色权限已验证。
- [ ] `/etc/cloudledger/ops.env`、`server.toml`、`rclone.conf` 和 Origin 私钥权限正确。
- [ ] `/health`、`/ready`、Caddy、证书、审计链和管理端 SSH 隧道均已实测。
- [ ] 第一份九文件完整备份已上传、下载、逐字节比对并通过校验。
- [ ] 临时数据库真实恢复演练通过，且临时数据库已删除。
- [ ] 8 个 systemd 单元已安装；健康/防火墙 timer 已启用，备份/恢复 timer 仅在前置验收后启用。
- [ ] crypt 密码、OAuth 恢复信息、GHCR tag 和本手册版本已在服务器外保存。
- [ ] Android/Web 发布前 `frontend/public/config.js` 已改为生产 API 域名。

相关资料：

- [安全加固与角色模型](security-hardening.md)
- [后端数据模型](backend-data-model.md)
- [Compose 配置](../deploy/docker-compose.yml)
- [运维脚本](../deploy/cloudledger-ops.sh)
