# 虾虾发布助手桌面端

这是 `liuk` 项目的 Tauri 桌面端发布助手。它提供图形界面来调用当前项目里的 `虾虾发布助手/xiaxia_publish.py`，用于检查、发布、查看代码变动和从最近 5 次 commit 回滚服务器代码。

## 当前能力

- 显示当前项目目录、Git 分支和代码变动
- 显示 `git status --short`
- 显示 `git diff --stat`
- 显示最近 5 次 commit
- 一键快速检查
- 一键完整检查
- 一键发布到服务器
- 选择最近 commit 后一键回滚服务器代码
- 发布过程日志实时显示
- 简单进度条和当前状态
- 发布和回滚已改为 Rust 原生 SSH/SFTP，不再依赖系统 `ssh/scp`

## 检查速度

桌面端的 `检查` 是原生快速检查，主要确认项目目录、Git、必要文件和 SSH key 是否正常，通常会很快完成。

`完整检查` 是兼容旧脚本的可选功能，会调用网站项目里的 `虾虾发布助手/xiaxia_publish.py check`，会额外检查页面 JavaScript、后端 Python 导入、缓存使用等。

## Windows 依赖变化

当前升级后，桌面端发布和回滚不再需要这些系统命令：

- Python
- Node.js
- OpenSSH Client
- `ssh`
- `scp`

当前仍然需要：

- GitHub Token
- 服务器 SSH 私钥

也可以在应用里点 `保存免输入配置`，把 GitHub Token、SSH 私钥路径或服务器密码保存到当前电脑的本机加密配置里。加密文件不在项目目录里，不会被 Git 提交。

保存位置：

- macOS：`~/Library/Application Support/xiaxia-publish-assistant/secrets.enc.json`
- Windows：`%APPDATA%\xiaxia-publish-assistant\secrets.enc.json`

说明：这是面向单人使用的本机加密，目的是避免误提交和避免明文落盘，不等同于企业级密钥管理。拿到这台电脑账号权限的人仍可能通过应用使用这些凭据。

## 本机开发

当前机器需要先安装 Rust，因为 Tauri 的桌面壳需要 Rust 编译：

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

然后在本目录执行：

```sh
npm install
npm run tauri:dev
```

项目已内置 Cargo 国内镜像配置：

```text
.cargo/config.toml
```

如果 Rust 依赖下载慢，Cargo 会优先走中科大镜像。

## 打包

Mac 打包：

```sh
npm run tauri:build
```

Windows 版本需要在 Windows 电脑或 Windows CI 上打包：

```sh
npm install
npm run tauri:build
```

也可以使用 GitHub Actions 自动打包 Windows 安装包。把这个桌面端项目推到 GitHub 后，在仓库的 `Actions` 页面手动运行 `build-windows`，完成后下载 `xiaxia-windows-installer` artifact。

已内置工作流：

```text
.github/workflows/build-windows.yml
```

## 说明

这个桌面端目前复用网站项目里的发布脚本，因此目标电脑仍需要满足原发布脚本的运行条件，包括 GitHub 权限和服务器 SSH key。后续可以继续把 Git、SSH/SFTP、检查逻辑内置到 Rust 后端里，逐步减少目标电脑需要安装的环境。
