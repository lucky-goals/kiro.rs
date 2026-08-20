# Railway 部署指南

## 🚀 快速部署

### 方法 1：Railway CLI

```bash
# 安装 CLI
npm install -g @railway/cli

# 登录
railway login

# 在项目目录部署
cd /path/to/kiro.rs
railway init
railway up
```

### 方法 2：GitHub 连接

1. 推送代码到 GitHub
2. 访问 [Railway Dashboard](https://railway.app/dashboard)
3. "New Project" → "Deploy from GitHub repo"
4. 选择仓库，自动开始构建

## ⚙️ 必要配置

### 1. Volume 持久化存储

**重要**：Railway 容器重启会清空数据，必须配置 Volume！

在 Railway Dashboard：
1. 进入服务 → "Volumes" 标签
2. "Add Volume"
3. **Mount Path**: `/app/config`
4. 保存并重新部署

### 2. 环境变量（可选）

```bash
# Railway 自动注入，无需手动设置：
# PORT - 动态分配的端口
# HOST - 已自动设为 0.0.0.0

# 可选配置：
KIRO_API_KEY=ksk_xxx        # 最高优先级凭据
GITHUB_TOKEN=ghp_xxx        # 避免更新 rate limit
```

### 3. 获取自动生成的密钥

首次部署后查看日志：

```bash
railway logs | grep "apiKey\|adminApiKey"
```

或直接读取配置：

```bash
railway run cat /app/config/config.json
```

## 🌐 访问服务

Railway 自动分配域名：`https://xxx.up.railway.app`

- **API**: `/v1/messages`, `/v1/chat/completions`
- **Admin UI**: `/admin`
- **Models**: `/v1/models`

测试连接：

```bash
curl https://your-domain.up.railway.app/v1/models \
  -H "x-api-key: YOUR_API_KEY"
```

## 📝 代码变更说明

已修改 `src/main.rs` 支持 Railway：

- ✅ 自动读取 `PORT` 环境变量
- ✅ 自动读取 `HOST` 环境变量
- ✅ 默认 `host` 设为 `0.0.0.0`（容器友好）

## 🔧 配置文件

首次运行自动生成 `/app/config/config.json`：

```json
{
  "host": "0.0.0.0",
  "port": 8990,
  "apiKey": "sk-kiro-rs-xxx",
  "adminApiKey": "sk-admin-xxx",
  "region": "us-east-1"
}
```

`port` 会被 `PORT` 环境变量覆盖。

## 🐛 故障排查

### 服务无法启动
```bash
railway logs
```

### Volume 数据丢失
确认 Volume 挂载点为 `/app/config`

### 无法访问
确保 Railway 分配了公开域名

## 💰 成本估算

- 开发者计划：$5/月（含 $5 额度）
- 轻量级使用：约 $2-10/月

## 📚 完整文档

详见 [README.md](README.md)
