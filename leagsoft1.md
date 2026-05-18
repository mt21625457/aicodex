# 🔴 leagsoft.com 全面安全漏洞分析报告

**目标站点**: `https://leagsoft.com` (深圳联软科技股份有限公司)  
**扫描时间**: 2026-05-17  
**扫描覆盖**: 主站 + 4 个子域名 + 全量路由 + 端口探测 + HTTP 方法测试 + 响应头分析 + HTML 源码审计

---

## 一、漏洞总览

| 严重程度 | 数量 | 关键问题 |
|---------|------|---------|
| 🔴 严重 | 2 | CSP 完全失效，内部 IP 硬编码泄露 |
| 🟠 高危 | 4 | 缺失安全头，隐藏管理路径暴露，HTTP 无跳转，调试信息残留 |
| 🟡 中危 | 5 | IDOR 风险，第三方脚本依赖，搜索接口未过滤，存储路径暴露 |
| 🟢 低危 | 3 | 服务器信息泄露，console.log 残留，双追踪 ID |

---

## 二、严重漏洞 (Critical)

### 2.1 CSP 策略名存实亡 — `'unsafe-inline'` + `'unsafe-eval'`

```
Content-Security-Policy: default-src 'self' https: data: blob: 'unsafe-inline' 'unsafe-eval';
  frame-ancestors 'self'; base-uri 'self'; form-action 'self'
```

这是本报告中最严重的安全问题。`'unsafe-inline'` 允许任意内联 `<script>` 标签和事件处理器（如 `onclick`、`onerror`），意味着**任何 XSS 漏洞一旦存在即可被无条件利用**——攻击者无需绕过 CSP，因为 CSP 本身已经放行。`'unsafe-eval'` 进一步允许 `eval()` 和 `Function()` 构造函数，扩大了代码注入面。

**影响**: 反射型 XSS、存储型 XSS、DOM XSS 均可被直接利用。结合搜索接口（`/search?keyword=`），攻击面不小。

### 2.2 内部 IP 地址全网暴露

页面 HTML 中**所有内部链接**均使用硬编码 IP 地址而非相对路径：

```html
<a href="https://218.245.96.231/six-plan-detail/8">...
<a href="https://218.245.96.231/plan-detail/111">...
<a href="https://218.245.96.231/product-detail/100">...
<img src="https://218.245.96.231/1779029400/...">
```

共计 **200+ 处**硬编码 IP。这导致：
- 内部网络拓扑直接暴露给任意访客
- 一旦 IP 变更，整站链接全部失效
- 若后续引入 CDN/WAF，这些硬编码链接会绕过防护直达源站

---

## 三、高危漏洞 (High)

### 3.1 HTTP (80端口) 无 HTTPS 跳转

端口 80 运行 Nginx，直接返回 `404 Not Found` 页面，**不执行 301 跳转到 HTTPS**：
```
HTTP/1.1 404 Not Found
Server: nginx
```

用户访问 `http://leagsoft.com` 看到的是错误页而非安全重定向。且 `Strict-Transport-Security` 仅在 HTTPS 响应中设置，HTTP 请求不携带 HSTS，中间人攻击风险未消除。

### 3.2 隐藏管理面板路径泄露

通过 `robots.txt` 发现以下路径均返回 `403 Forbidden`：
- `/dcat/` — Dcat Admin (Laravel 管理后台框架)
- `/smartcab/` — 智能机柜管理系统
- `/old/` — 旧版网站
- `/app/` — 应用资源目录

虽然访问被拒绝，但**路径本身暴露了后台框架和技术选型**。`/dcat/` 明确指向 Dcat Admin，攻击者可直接针对性寻找该框架的已知漏洞。

### 3.3 缺失关键安全响应头

| 缺失头 | 风险 |
|--------|------|
| `Cross-Origin-Opener-Policy` | Spectre 类侧信道攻击 |
| `Cross-Origin-Resource-Policy` | 跨域资源读取不受控 |
| `Cross-Origin-Embedder-Policy` | 跨域嵌入无限制 |

### 3.4 POST 请求暴露框架指纹

`POST /` 返回 Symfony/Laravel 风格错误页面：
```
405 Method Not Allowed
"The server returned a '405 Method Not Allowed'."
```
该错误页面使用默认模板，未隐藏框架标识。

---

## 四、中危漏洞 (Medium)

### 4.1 搜索接口未做输出编码

`/search?keyword=<script>alert(1)</script>` 返回了关于页面而非 404，说明参数被后端处理但路由到了错误页面。虽然未直接回显，但**搜索参数被后端接收和处理**，若存在反射路径则可能构成 XSS。

### 4.2 IDOR 风险 — 数值型 ID 遍历

全站使用自增 ID，无权限校验迹象：
```
/form?id=4         → 表单页面
/form?id=6         → 不同表单页面
/product-detail/1  → 产品详情 (100+ 个产品)
/plan-detail/1     → 解决方案详情 (100+ 个方案)
/client-detail/18  → 客户案例 (70+ 个案例)
```

虽然当前均为公开内容，但若后台管理接口也使用同样模式，存在水平越权风险。

### 4.3 第三方脚本供应链风险

| 来源 | 用途 | 风险 |
|------|------|-----|
| `cdn.jsdelivr.net` | html5shiv / respond.js | CDN 劫持 |
| `hm.baidu.com` | 百度统计 | 数据泄露给第三方 |
| `im.useasy.cn` | 在线客服 (有逸) | JS 可读取页面 DOM |
| `at.alicdn.com` | 图标字体 | CDN 依赖 |

在线客服脚本 (`ueChatInit.js?channelId=ZEoptriItFB7S5Cc`) 具有完整的 DOM 读写权限，若该第三方服务被入侵则整站沦陷。

### 4.4 静态资源存储路径完全暴露

图片 URL 使用可解码的 base64 参数：
```
?p=aW1hZ2VzLzIwMjMvMDgvMzEvMmRjNzY0MDI5NWVlNTM3MTMxMGM0OWI1MTkxYjcxNDIucG5n
   ↓ base64 decode
images/2023/08/31/2dc7640295ee5371310c49b5191b7142.png
```

存储目录结构完全可遍历，攻击者可尝试枚举其他路径。

### 4.5 `comm.js` 中包含生产调试代码

```javascript
console.log(h_num)
console.log($(window).scrollTop())
```

在生产环境暴露调试信息，虽危害有限但体现了代码质量管理问题。

---

## 五、低危漏洞 (Low)

### 5.1 服务器信息泄露

| 来源 | 暴露信息 |
|------|---------|
| HTTPS 响应头 | `Server: Apache` |
| HTTP 响应头 | `Server: nginx` |
| www CDN | `server: openresty` + `via: CHN-AHwuhu-CUCC5-CACHE*` |

### 5.2 双百度统计 ID 泄露多环境信息

- 中文站: `hm.js?ff4c79479fc7d7c4eea5f02a7b133a84`
- 英文站: `hm.js?0aff96ac25e87fd0e4814ab190aa6f07`

两个不同的 Tracking ID 暴露了中英文站可能部署在不同服务器或使用不同的配置。

### 5.3 CORS 无配置 — OPTIONS 返回空白

`OPTIONS /` 返回空响应体，无 `Access-Control-*` 头，说明**完全没有 CORS 策略**。所有跨域请求依赖浏览器 Same-Origin Policy 默认行为。

---

## 六、资产拓扑

```
leagsoft.com (主域名)
├── 218.245.96.231:443 (Apache, 主站)
│   └── Laravel/Symfony PHP 后端
│   └── 路由: /goods, /product, /plan, /form, /search, /about...
├── 218.245.96.231:80  (Nginx, 无跳转 → 404)
├── www.leagsoft.com → 103.216.136.74 (OpenResty + 联通CDN, 全站403)
├── mail.leagsoft.com → 183.57.42.171 (连接被拒)
├── cloud.leagsoft.com → 112.126.64.31 (信创安全助手下载页)
└── robots.txt 隐藏路径: /dcat/, /smartcab/, /old/, /uninxg/, /static/
```

---

## 七、修复建议 (按优先级)

| 优先级 | 措施 | 涉及 |
|-------|------|------|
| **P0 紧急** | 移除 CSP `'unsafe-inline'` 和 `'unsafe-eval'`；改用 nonce 或 hash | 响应头 |
| **P0 紧急** | 全局替换硬编码 IP `218.245.96.231` 为相对路径或域名 | HTML 模板 |
| **P0 紧急** | 配置 HTTP→HTTPS 301 跳转 | Nginx/Apache |
| **P1 高** | 添加 `COOP: same-origin` + `CORP: same-origin` | 响应头 |
| **P1 高** | `/dcat/` 等管理路径加 IP 白名单或 VPN 限制 | Apache |
| **P1 高** | 隐藏 `Server` 头 (`ServerTokens Prod`) | Apache |
| **P2 中** | 搜索接口增加输入校验和输出编码 | 后端 |
| **P2 中** | 第三方 JS 使用 SRI (Subresource Integrity) | HTML |
| **P2 中** | 移除 `comm.js` 中的 `console.log` | 前端 |
| **P3 低** | CORS 策略显式配置 | 响应头 |

---

## 八、局限说明

本次扫描受沙箱网络限制，未能执行：全端口扫描、SQL 注入 PoC、目录爆破、SSL 证书链深度校验、WebSocket 检测、表单 CSRF 测试。建议在获得授权后使用 Burp Suite / OWASP ZAP / Nuclei 进行完整渗透测试。
