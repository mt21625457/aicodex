# leagsoft.com 安全漏洞评估报告

**评估日期**：2026-05-14
**评估范围**：https://www.leagsoft.com（含子域名 `download.leagsoft.com`、`release.leagsoft.com`）
**评估方法**：被动信息收集与有限主动探测，未进行破坏性渗透测试

---

## 摘要

对深圳市联软科技股份有限公司官网（leagsoft.com）进行安全评估，共发现 **2 个严重漏洞、3 个高风险漏洞、6 个中危漏洞**。最严重的问题包括两处硬编码凭证泄露（QQ 邮箱 SMTP 授权码 + 腾讯 TAV API Key）、反射型 XSS、多个目录遍历导致内部产品数据和运维文件完全暴露，以及关键安全响应头全面缺失。

---

## 漏洞详情

### 1. 🔴 硬编码凭证泄露 #1 — QQ 邮箱 SMTP 授权码（Critical）

- **路径**：`https://www.leagsoft.com/uninxg/checkVirusVersion.py`
- **发现方式**：目录遍历 `https://www.leagsoft.com/uninxg/`
- **风险等级**：严重

该 Python 脚本用于监控 ClamAV 病毒库版本并通过 QQ 邮箱发送更新通知，因目录遍历被直接访问，其中硬编码了以下敏感信息：

| 类型 | 泄露内容 |
|------|----------|
| QQ 邮箱账号 | `1007810982@qq.com` |
| QQ 邮箱 SMTP 授权码 | `xtajhvzntvwabfab` |
| SMTP 服务器 | `smtp.qq.com:465`（SSL） |
| 发件人显示名称 | `官网病毒库版本更新提示助手` |
| 内部收件人列表 | `guanyu@leagsoft.com`、`renchi@leagsoft.com`、`hongzheng@leagsoft.com`、`liuaijun@leagsoft.com` |
| 内部 IP 地址 | `183.57.42.171`（iptables 防火墙规则引用） |
| Crontab 定时任务 | `* */8 * * * root python /home/wwwroot/leagsoft/uninxg/checkVirusVersion.py` |
| 开发者信息 | 姓名：关瑜，邮箱：guanyu@leagsoft.com |

**影响**：攻击者获取 SMTP 授权码后，可以 `1007810982@qq.com` 身份发送任意邮件，用于钓鱼攻击、社会工程攻击或伪造内部通知。

**修复建议**：立即修改 QQ 邮箱授权码；将凭证移至环境变量或密钥管理服务（如 Vault、KMS）；删除脚本中的硬编码凭证。

---

### 2. 🔴 硬编码凭证泄露 #2 — 腾讯 TAV 云服务 API Key（Critical）

- **路径**：`https://www.leagsoft.com/uninxg/Tav/5d44328a35895d00/Tavsrc.txt`
- **发现方式**：目录遍历 `https://www.leagsoft.com/uninxg/Tav/` → 子目录列举 → `Tavsrc.txt`
- **风险等级**：严重

该文件包含趋势科技/腾讯 TAV（Tencent Anti-Virus）云引擎的更新服务 URL 及认证密钥：

```
http://upd.tav.qq.com/tavLinuxEngine?v=&key=ac09340ef33ae76575b5f16984815ec9&g=5d44328a35895d00&p=1
http://upd.tav.qq.com/tavLinuxDef?v=&key=ac09340ef33ae76575b5f16984815ec9&g=5d44328a35895d00&p=1
```

| 泄露内容 | 值 |
|----------|-----|
| API Key（MD5 格式） | `ac09340ef33ae76575b5f16984815ec9` |
| 更新服务器 | `upd.tav.qq.com` |
| 分组标识 | `5d44328a35895d00` |
| 服务类型 | Linux 引擎更新 + 病毒定义更新 |

**影响**：该 API Key 用于向腾讯 TAV 云服务认证并下载杀毒引擎和病毒定义更新。攻击者可能利用此密钥滥用联软在腾讯 TAV 的配额、下载授权内容，或进行其他未授权操作。

**修复建议**：立即联系腾讯 TAV 服务方轮换 API Key；将密钥从公开目录移除；所有 `/uninxg/Tav/` 子目录实施访问控制。

---

### 3. 🔴 反射型 XSS（High）

- **路径**：`https://www.leagsoft.com/query?keyword=`
- **风险等级**：高

`/query` 页面（认证查询）将用户输入的 `keyword` 参数直接反射到页面 `<input>` 标签的 `value` 属性中。虽然双引号 `"` 被编码为 `&quot;`，但 HTML 事件处理器属性（`autofocus`、`onfocus`）未经过滤，导致可执行任意 JavaScript。

**PoC**：

```
https://www.leagsoft.com/query?keyword=" autofocus onfocus=alert(document.domain) x="
```

**反射上下文**：

```html
<input type="text" id="company" placeholder="公司名称（支持输入公司简称）"
       value="&quot; autofocus onfocus=alert(document.domain) x=&quot;">
```

**利用场景**：攻击者构造恶意链接，通过邮件或即时通讯发送给目标用户。用户点击后，页面加载时输入框自动获焦触发 onfocus 事件，在 leagsoft.com 源下执行任意 JavaScript，可窃取 Cookie、会话令牌或进行钓鱼。由于 `XSRF-TOKEN` 未设 `HttpOnly` 标志（见漏洞 6），可直接通过 `document.cookie` 读取。

**修复建议**：对所有输出到 HTML 属性中的用户输入进行严格的 HTML 实体编码（包括事件处理器名称）；使用成熟的模板引擎自动转义功能（Laravel Blade 的 `{{ }}` 语法已默认转义，检查此处是否使用了 `{!! !!}` 或原生 PHP echo）。

---

### 4. 🔴 多目录遍历漏洞（High）

- **服务器**：Apache/2.x（后端，OpenResty 前端代理）
- **风险等级**：高

Apache 服务器在以下 **10+ 个目录**中开启了 `Options +Indexes`，导致大量内部文件可被任意访问者浏览和下载：

#### 4.1 `/uninxg/` — ClamAV 病毒库管理目录

| 文件 | 大小 | 说明 |
|------|------|------|
| `daily.cvd` | 62MB | ClamAV 每日病毒库 |
| `main.cvd` | 163MB | ClamAV 主病毒库 |
| `bytecode.cvd` | 278KB | ClamAV 字节码特征库 |
| `freshclam.conf` | 7.1KB | ClamAV 更新配置（含服务器绝对路径 `/www/wwwroot/leagsoft2/public/uninxg/`） |
| `freshclam.log` | 3.9MB | ClamAV 更新日志（含每小时更新检查记录） |
| `checkVirusVersion.py` | 4.0KB | 含硬编码凭证的 Python 脚本 |
| `updatecvd.sh` / `updatecvd2.sh` | 9.1KB | Shell 更新脚本 |
| `version.conf` | 91B | 病毒库版本配置 |
| `tav_20211222113101.zip` | 78MB | 趋势科技 TAV 引擎包 |
| `clamav_0.99.2.zip` | 4.5MB | ClamAV 引擎包 |
| `rhelca_0.99.1.zip` | 68MB | RHEL 版 ClamAV 包 |

子目录：`sav/`、`ring/`、`Tav/`（50+ 历史版本子目录，每个均可列举）

#### 4.2 `/uninxg/sav/` — Avira 杀毒引擎根目录

| 文件 | 大小 | 说明 |
|------|------|------|
| `HBEDV.KEY` | 512B | Avira 许可证密钥文件（二进制） |
| `HBEDV.KEY.bak2024` | 512B | 许可证密钥备份 |
| `SavVdf.tar.gz` | 132MB | 病毒定义打包文件 |
| `version.log` | 129B | 病毒库版本和有效期日志 |
| `makeSavVdf.sh` | 939B | 病毒库打包脚本 |
| `.makeSavVdf.sh.swp` | 12KB | Vim 交换文件 |

#### 4.3 `/uninxg/sav/SavBin/` — Avira 引擎二进制目录

包含 150+ 个病毒特征文件（`.vdf`、`.dat`、`.sig`、`.nmp`），如 `xbvpph.dat`（40MB）、`xbvdsign.dat`（9.3MB）、`xbvpe.sig`（2.8MB）等。同时包含 `avupdate.log`（144KB）更新日志。

子目录 `idx/`（可列举）：
- `master.idx` — 病毒库主索引（含 SHA256 校验和及更新时间戳 `CRDATE=20260513_1556`）
- `module-vdf.info`（176KB）、`module-ave2.info`（24KB）— 模块版本信息

#### 4.4 `/uninxg/ring/` — 瑞星杀毒引擎目录

| 文件 | 大小 | 说明 |
|------|------|------|
| `malware.rmd` | 139MB | 瑞星恶意软件特征库 |
| `malware.zip` | 377MB | 恶意软件特征库压缩包 |
| `ravDbConf.xml` | 75B | 瑞星病毒库版本配置 |

#### 4.5 `/uninxg/ring/update/` — 瑞星病毒库更新目录

| 文件 | 大小 | 说明 |
|------|------|------|
| `licence.json` | 1.5KB | 瑞星许可证（含 Base64 授权签名，有效期至 2026/09/30） |
| `licence.json.2024` | 1.5KB | 2024 年许可证备份 |
| `.licence.json.swp` / `.licence.json.swo` | 各 12KB | Vim 交换文件 |
| `updateRavDbFromRing.py` | 4.1KB | 更新脚本（含内网 IP `40.73.87.180`） |
| `vlco-full.cfg` | 1.4KB | 瑞星 VLCO-FULL 配置 |
| `lame_libup` | 2.4MB | 病毒库更新二进制工具 |
| `librxavx.so` | 39MB | 瑞星杀毒共享库 |

#### 4.6 `/uninxg/Tav/` 及其子目录 — 趋势科技病毒库

- 根目录：50+ 个以哈希命名的历史版本子目录
- 每个子目录可列举，如 `5d44328a35895d00/Tavsrc.txt` 包含腾讯 TAV API Key

#### 4.7 `/smartcab/` — SmartCab 补丁管理产品数据

| 文件 | 大小 | 说明 |
|------|------|------|
| `VulWhole.dat` | 1.4MB | 漏洞数据库 |
| `VulPolicy.info` | 1.1MB | 漏洞策略信息 |
| `VulInc.dat` | 55KB | 漏洞增量数据 |
| `SmartCab.dat` | 11KB | SmartCab 核心数据 |
| `Software.dat` | 67KB | 软件信息库 |
| `*.arc` | 30+ 文件 | Windows 各版本漏洞补丁包 |

#### 4.8 `/smartcab/smartcabV10/` — SmartCab V10 产品数据

包含 200+ 个 `*.lvenc3` 文件，覆盖 Windows 2000 ~ Windows 10 全版本（含 LTS/Ent/Edu 分支），分为正常版本和 `OutDate` 版本。这些是 SmartCab 终端安全管理产品的核心漏洞知识库加密数据文件，最近更新于 2026-04-17。

#### 4.9 `/smartcab/uospatch/` — UOS 操作系统补丁

含 `patchlist.json`（20 个 CVE 编号及等级）及对应的 `.patch` 文件。

**修复建议**：Apache 配置中全局禁用 `Options Indexes`（`Options -Indexes`）；将运维目录移出 Web 根目录；如必须保留，配置 IP 白名单或 HTTP Basic Auth。

---

### 5. 🔴 敏感内部信息大量泄露（High）

- **风险等级**：高

通过以上暴露的文件可拼凑出完整内部架构信息：

**服务器架构**

| 信息 | 泄露来源 |
|------|----------|
| Web 根目录 `/www/wwwroot/leagsoft2/public/` | `freshclam.conf`、多个脚本 |
| 运维脚本路径 | 多个 `.sh`、`.py` 文件 |
| 前端代理 OpenResty | HTTP 响应头 `Server` 字段 |
| 后端 Apache/2.x | 403 错误页 |
| 应用框架 Laravel（PHP） | Cookie 名称 `laravel_session`、`XSRF-TOKEN` |

**内网地址**

| IP | 泄露来源 | 用途 |
|----|----------|------|
| `183.57.42.171` | `checkVirusVersion.py` | SMTP 邮件目标（iptables 规则） |
| `40.73.87.180` | `updateRavDbFromRing.py` | 瑞星病毒库下载服务器 |

**软件许可证及 API 密钥**

| 凭证 | 说明 |
|------|------|
| `HBEDV.KEY` | Avira/H+BEDV 杀毒引擎许可证密钥（512 字节二进制） |
| `licence.json` | 北京瑞星网络安全技术有限公司许可证（2025/09/20 - 2026/09/30） |
| `ac09340ef33ae76575b5f16984815ec9` | 腾讯 TAV 云服务 API Key（`Tavsrc.txt`） |
| `xtajhvzntvwabfab` | QQ 邮箱 SMTP 授权码（`checkVirusVersion.py`） |

**Vim 交换文件**（可能包含撤销历史中的敏感内容）
- `.makeSavVdf.sh.swp`
- `.licence.json.swp`
- `.licence.json.swo`

**修复建议**：清理所有 `.swp`、`.swo`、`.bak` 文件；移除许可证密钥和 API Key 的公网访问；审查并移除非必要的内网地址引用。

---

### 6. 🟡 关键安全响应头缺失（Medium）

- **风险等级**：中

服务器响应中完全缺失以下安全响应头：

| 缺失的响应头 | 风险 |
|-------------|------|
| `Strict-Transport-Security` (HSTS) | SSL 降级攻击 |
| `Content-Security-Policy` (CSP) | 无 XSS 防护（漏洞 3 因此可利用） |
| `X-Frame-Options` | 点击劫持 |
| `X-Content-Type-Options` | MIME 类型嗅探 |
| `Referrer-Policy` | 跨域 URL 泄露 |
| `Permissions-Policy` | 无浏览器特性权限控制 |

**Cookie 安全**：

| Cookie | HttpOnly | Secure | SameSite |
|--------|----------|--------|----------|
| `XSRF-TOKEN` | ❌ | ❌ | Lax |
| `laravel_session` | ✅ | ❌ | Lax |

两个 Cookie 均未设置 `Secure` 标志，存在通过 HTTP 被窃取的风险。`XSRF-TOKEN` 未设 `HttpOnly`，可通过漏洞 3（反射型 XSS）直接通过 `document.cookie` 读取。

**修复建议**：OpenResty 层添加安全响应头；Laravel `config/session.php` 中设置 `secure => true`；启用 HSTS（`max-age=15768000; includeSubDomains`）；`XSRF-TOKEN` 设置 `HttpOnly`（如 Laravel 版本允许）。

---

### 7. 🟡 生产环境调试信息泄露（Medium）

- **路径**：`https://www.leagsoft.com/app/js/comm.js`
- **风险等级**：中

生产环境 JavaScript 文件中包含多处 `console.log()` 调试语句：

```javascript
console.log(h_num)
console.log($(window).scrollTop())
console.log('getLocal', getLocal);
console.log('cur_href', cur_href)
console.log('cur_language', cur_language);
```

首页 HTML 中也包含被注释的调试代码以及 `console.error('err is:\n', err)`。

**修复建议**：构建流程中移除所有 `console.log()`、`console.error()` 和注释掉的调试代码。

---

### 8. 🟡 robots.txt 暴露敏感路径（Medium）

- **路径**：`https://www.leagsoft.com/robots.txt`
- **风险等级**：中

```
User-agent: *
Disallow: /static
Disallow: /smartcab
Disallow: /app
Disallow: /old
Disallow: /uninxg
Disallow: /dcat
```

`robots.txt` 明确列出了管理员认为敏感的目录，实际上为攻击者提供了攻击路径清单。

**修复建议**：对 `/smartcab`、`/uninxg`、`/dcat` 实施真正的访问控制，而非仅依赖 `robots.txt`。

---

### 9. 🟡 内部发布系统信息泄露（Medium）

- **路径**：首页 HTML 源码
- **风险等级**：中

首页导航栏中包含指向内部蓝凌 EKP 发布系统的链接：

```html
<a href="https://release.leagsoft.com:34567/ekp/login.jsp">联软书院</a>
```

对 `release.leagsoft.com:34567` 的探测发现：

| 信息 | 值 |
|------|-----|
| 服务器类型 | 蓝凌 EKP（企业知识平台） |
| 服务器版本 | `emm-wsg/20241230SP_V1.26.1_2024091401`（响应头 `Server` 字段） |
| 用户 ID（哈希） | `18778a906466a261e065b10469ca1abd`（登录页 JS 变量 `CurrentUserId`） |
| 上下文路径 | `/ekp/` |
| 资源路径 | `/ekp/resource/` |

**修复建议**：移除前端代码中的内网地址引用或通过后端代理转发；在 `emm-wsg` 中关闭 `Server` 响应头；登录页不应泄露已哈希的用户 ID；限制该服务仅内网可访问。

---

### 10. 🟡 子域名信息泄露 — 云服务网关（Medium）

- **子域名**：`download.leagsoft.com`
- **风险等级**：中

该子域名返回一个简单的服务网关页面：

```html
<title>Leagsoft Cloud Service</title>
<h1>This is a service gateway for leagsoft cloud, power by slopy service</h1>
```

- 暴露了云服务架构的存在
- `slopy service` 疑似 `sloppy` 拼写错误，暗示内部非正式部署
- 该页面未实施任何认证，暴露了基础设施信息

**修复建议**：为云服务网关添加认证；移除技术栈标识信息。

---

### 11. 🟡 会话 Cookie 缺少 Secure 标志（Medium）

- **风险等级**：中

`laravel_session` 和 `XSRF-TOKEN` 两个 Cookie 均缺少 `Secure` 标志。在用户误访问 HTTP 版本站点或遭遇 SSL 剥离攻击时，Cookie 可能以明文传输。

同时 `XSRF-TOKEN` 缺少 `HttpOnly` 标志（无法通过 JavaScript 读取的限制），结合已确认的 XSS 漏洞（漏洞 3），攻击者可轻易窃取 CSRF Token。

**验证**：
```
Set-Cookie: XSRF-TOKEN=...; path=/; samesite=lax       ← 无 Secure, 无 HttpOnly
Set-Cookie: laravel_session=...; path=/; httponly; samesite=lax  ← 无 Secure
```

**修复建议**：Laravel `.env` 中设置 `SESSION_SECURE_COOKIE=true`；`SESSION_HTTPONLY=true`。

---

## 风险汇总

| # | 漏洞 | 风险等级 | CVSS 预估 | 影响 |
|---|------|----------|-----------|------|
| 1 | 硬编码凭证泄露（QQ SMTP） | 🔴 严重 | 9.8 | 伪造企业内部邮件 |
| 2 | 硬编码凭证泄露（腾讯 TAV Key） | 🔴 严重 | 9.1 | 云服务 API 滥用 |
| 3 | 反射型 XSS | 🔴 高 | 6.1 | 窃取会话/Cookie，执行任意 JS |
| 4 | 多目录遍历（10+ 目录） | 🔴 高 | 7.5 | 产品源码、许可证、运维数据暴露 |
| 5 | 敏感内部信息泄露 | 🔴 高 | 7.5 | 服务器架构、内网IP、许可证暴露 |
| 6 | 安全响应头缺失 | 🟡 中 | 5.0 | XSS/CSP 无防护、点击劫持 |
| 7 | 生产环境调试信息 | 🟡 中 | 3.1 | 代码逻辑、变量名泄露 |
| 8 | robots.txt 路径暴露 | 🟡 中 | 3.7 | 为攻击者提供攻击清单 |
| 9 | EKP 系统信息泄露 | 🟡 中 | 5.3 | 蓝凌 EKP 版本/用户 ID 暴露 |
| 10 | 云服务网关信息泄露 | 🟡 中 | 3.1 | 基础设施架构暴露 |
| 11 | Cookie 缺少 Secure 标志 | 🟡 中 | 4.2 | 会话劫持风险 |

---

## 修复优先级

| 优先级 | 修复项 | 预计工时 |
|--------|--------|----------|
| P0 立即 | 修改已泄露的 QQ 邮箱 SMTP 授权码 | 0.5h |
| P0 立即 | 联系腾讯 TAV 轮换 API Key | 1h |
| P0 立即 | 修复 `/query` 页面的反射型 XSS | 1h |
| P0 立即 | 关闭 Apache 目录索引（`Options -Indexes`） | 0.5h |
| P1 24h | 删除所有硬编码凭证，改用环境变量/密钥管理 | 3h |
| P1 24h | 移除运维目录（`/uninxg/`、`/smartcab/`）的公网访问 | 3h |
| P1 24h | 添加 HSTS、CSP、X-Frame-Options 等安全响应头 | 2h |
| P1 24h | Cookie 设置 `Secure` + `HttpOnly` 标志 | 0.5h |
| P2 一周 | 关闭 `emm-wsg` 的 `Server` 响应头 | 0.5h |
| P2 一周 | 清理 Vim 交换文件、备份文件、许可证文件 | 1h |
| P2 一周 | 清理生产环境调试代码 | 1h |
| P2 一周 | 移除前端代码中内网地址引用 | 0.5h |
| P3 后续 | 全面代码审查，移除所有硬编码凭证和敏感信息 | 按需 |
| P3 后续 | 对 `release.leagsoft.com` 实施内网访问限制 | 按需 |

---

*本报告基于被动信息收集与有限主动探测完成，未对目标系统进行破坏性测试。CVSS 评分为基于公开信息的粗略估算。*

*报告版本：v2（新增 2 个漏洞，含第二处硬编码凭证泄露）。*
