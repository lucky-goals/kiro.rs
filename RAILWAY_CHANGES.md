# Railway 部署变更总结

## ✅ 已完成的修改

### 1. 代码修改 (src/main.rs)
添加了 Railway 环境变量支持：
- 自动读取 `PORT` 环境变量（Railway 动态分配）
- 自动读取 `HOST` 环境变量
- 保持向后兼容

### 2. 新增文件
- `railway.toml` - Railway 配置
- `.railwayignore` - 构建优化
- `RAILWAY_DEPLOY.md` - 部署指南
- `README.md` - 添加 Railway 部署入口

### 3. 代码验证
✅ 编译通过 (cargo check)

## 🚀 快速部署步骤

### Railway CLI 方式：
```bash
railway login
railway init
railway up
```

### GitHub 方式：
1. 推送代码到 GitHub
2. Railway Dashboard → Deploy from GitHub

## ⚠️ 必须配置

### Volume 持久化（关键！）
Railway Dashboard → Volumes → Add Volume
- Mount Path: `/app/config`

### 查看 API Keys
```bash
railway logs | grep apiKey
# 或
railway run cat /app/config/config.json
```

## 📝 配置说明

### 自动生成
首次运行自动创建：
- config.json（含随机 apiKey 和 adminApiKey）
- credentials.json（空数组，通过 Admin UI 添加凭据）

### 环境变量（可选）
- `KIRO_API_KEY` - 注入最高优先级凭据
- `GITHUB_TOKEN` - 避免更新 rate limit
- `PORT` 和 `HOST` - Railway 自动注入

## 🌐 访问服务

Railway 分配域名：`https://xxx.up.railway.app`

- API: `/v1/messages`
- Admin UI: `/admin`
- Models: `/v1/models`

## 📚 详细文档

见 `RAILWAY_DEPLOY.md`
