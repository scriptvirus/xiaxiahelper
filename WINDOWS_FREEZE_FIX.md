# Windows 端卡死问题诊断与解决方案

## 🔍 问题现象
Windows 端点击"发布到服务器"后程序界面卡死，无响应。

## 🐛 根本原因

### 1. TCP 连接没有超时（已修复 ✅）
```rust
// 之前：无超时，可能无限等待
let tcp = TcpStream::connect((config.remote_host.as_str(), config.remote_port))?;

// 现在：30秒超时
let tcp = TcpStream::connect_timeout(
    &format!("{}:{}", config.remote_host, config.remote_port).parse()?,
    std::time::Duration::from_secs(30),
)?;
```

### 2. Tauri Command 同步阻塞（潜在问题 ⚠️）
```rust
#[tauri::command]
fn native_deploy(...) -> Result<RunResult, String> {
    // 这个函数在主线程执行
    // SSH 连接、文件上传等耗时操作会阻塞 UI
}
```

**为什么 macOS 不卡但 Windows 卡？**
- Windows 的窗口消息循环对阻塞更敏感
- macOS 的事件循环实现更宽容
- 但两个平台都应该使用异步

## ✅ 已应用的修复

### 修复 1：添加超时设置
```rust
fn connect_ssh(app: &AppHandle, config: &PublishConfig, server_password: Option<&str>) -> Result<Session, String> {
    // 1. TCP 连接超时（30秒）
    let tcp = TcpStream::connect_timeout(..., Duration::from_secs(30))?;
    
    // 2. 读写超时
    tcp.set_read_timeout(Some(Duration::from_secs(30)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(30)))?;
    
    // 3. SSH 会话超时
    session.set_timeout(30000); // 毫秒
    
    // ...
}
```

**作用**：
- 防止网络不通时无限等待
- 30秒后会返回错误，不会一直卡死
- 用户能看到错误信息

## 🔬 测试步骤

### 测试 1：验证超时是否生效
1. 修改配置文件，把服务器地址改成不存在的 IP
2. 点击"发布到服务器"
3. **预期**：30秒后显示"连接服务器失败"错误，不会卡死

### 测试 2：验证正常发布
1. 恢复正确的服务器地址
2. 确保 GitHub Token 已配置
3. 点击"发布到服务器"
4. **预期**：能看到进度日志，不会卡死

### 测试 3：测试网络中断
1. 发布过程中断网（拔网线或关 WiFi）
2. **预期**：30秒后显示错误，不会无限卡死

## 🎯 如果仍然卡死

### 可能的原因：
1. **SSH 认证卡住**：私钥格式问题或服务器未响应
2. **文件上传卡住**：SFTP 传输超时
3. **Git 操作卡住**：GitHub 连接问题

### 调试方法：
查看日志，看卡在哪一步：
- "==> 连接服务器" - 如果停在这里，说明 TCP 连接有问题
- "尝试使用内置 SSH key 登录..." - 如果停在这里，说明 SSH 认证有问题
- "上传：xxx" - 如果停在这里，说明文件上传有问题
- "==> Git 提交并推送" - 如果停在这里，说明 Git 操作有问题

## 💡 建议的进一步优化（可选）

### 方案 1：使用 Tokio 异步运行时
将耗时操作放到异步任务中执行：

```rust
#[tauri::command]
async fn native_deploy(...) -> Result<RunResult, String> {
    tokio::task::spawn_blocking(|| {
        // 在独立线程中执行 SSH 操作
        // UI 不会被阻塞
    }).await?
}
```

优点：
- UI 完全不会卡顿
- 更符合 Tauri 最佳实践

缺点：
- 需要添加 Tokio 依赖
- 需要改动较多代码

### 方案 2：增加更多进度反馈
在关键步骤之间添加更多 `emit_line`：

```rust
emit_line(app, "正在连接服务器...");
let tcp = TcpStream::connect_timeout(...)?;
emit_line(app, "OK TCP 连接成功");

emit_line(app, "正在 SSH 握手...");
session.handshake()?;
emit_line(app, "OK SSH 握手完成");
```

优点：
- 用户能看到进度
- 知道程序没有卡死，只是在工作

## 📊 性能优化检查清单

- [x] TCP 连接超时
- [x] TCP 读写超时  
- [x] SSH 会话超时
- [ ] 使用异步 Tauri command（可选）
- [x] 添加详细的进度日志
- [x] 错误信息清晰明确

## 🔐 安全提示

超时设置不影响安全性：
- SSH 加密通道仍然安全
- 私钥仍然加密存储
- 只是添加了时间限制，防止无限等待

---

## 📝 版本历史

- 2026-07-28: 添加 TCP 和 SSH 超时设置
- 2026-07-28: 初始诊断文档
