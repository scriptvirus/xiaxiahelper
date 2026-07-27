import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import "./styles.css";

const state = {
  projectPath: localStorage.getItem("xiaxia.projectPath") || "",
  commitMessage: localStorage.getItem("xiaxia.commitMessage") || "",
  githubToken: "",
  secretStatus: null,
  setupComplete: null, // null = loading, true = done, false = need setup
  setupError: "",
  running: false,
  activeAction: "",
  progress: 0,
  logs: [],
  status: null,
  commits: [],
  showHelp: false, // 是否显示帮助模态框
};

const steps = {
  quickCheck: ["快速检查"],
  check: ["检查电脑环境", "检查项目文件", "检查页面语法", "检查后端", "检查缓存", "检查 Git"],
  deploy: ["原生发布开始", "Git 提交并推送", "打包代码", "原生上传并部署服务器", "重启后端服务", "服务器本机验证"],
  rollback: ["原生回滚", "按 commit 打包", "原生上传并部署服务器", "重启后端服务", "服务器本机验证"],
};

const app = document.querySelector("#app");

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

function appendLog(line) {
  if (!line) return;
  state.logs.push(line);
  if (state.logs.length > 800) state.logs.shift();
  updateProgressFromLine(line);
  render();
  const box = document.querySelector(".log-box");
  if (box) box.scrollTop = box.scrollHeight;
}

function updateProgressFromLine(line) {
  const currentSteps = steps[state.activeAction] || [];
  const matchedIndex = currentSteps.findIndex((step) => line.includes(step));
  if (matchedIndex >= 0) {
    state.progress = Math.max(state.progress, Math.round(((matchedIndex + 1) / currentSteps.length) * 100));
  }
  if (line.includes("发布助手执行成功") || line.includes("OK  发布前检查全部通过")) {
    state.progress = 100;
  }
}

function setRunning(action, value) {
  state.activeAction = value ? action : "";
  state.running = value;
  if (value) {
    state.progress = 3;
    state.logs = [];
  }
  render();
}

async function refreshStatus() {
  try {
    if (state.projectPath) {
      state.status = await invoke("project_status", { projectPath: state.projectPath });
      state.commits = await invoke("recent_commits", { projectPath: state.projectPath });
    }
    state.secretStatus = await invoke("saved_secrets_status");
    const setup = await invoke("check_setup_complete");
    state.setupComplete = setup.complete;
  } catch (error) {
    state.status = { error: String(error), branch: "", status: "", diffStat: "" };
  }
  render();
}

async function browseProjectPath() {
  try {
    const selected = await open({ directory: true, multiple: false, title: "选择项目目录" });
    if (selected) {
      state.projectPath = selected;
      localStorage.setItem("xiaxia.projectPath", state.projectPath);
      await refreshStatus();
    }
  } catch (error) {
    appendLog(`选择目录失败：${error}`);
  }
}

async function initProject() {
  if (state.running) return;
  try {
    const selected = await open({ directory: true, multiple: false, title: "选择一个空文件夹来存放项目" });
    if (!selected) return;
    state.running = true;
    state.activeAction = "clone";
    state.progress = 3;
    state.logs = [];
    render();
    const resultPath = await invoke("clone_project", { targetDir: selected });
    state.projectPath = resultPath;
    localStorage.setItem("xiaxia.projectPath", state.projectPath);
    state.progress = 100;
    appendLog("OK  项目初始化完成，可以开始使用了");
  } catch (error) {
    appendLog(`初始化失败：${error}`);
  } finally {
    state.running = false;
    state.activeAction = "";
    await refreshStatus();
  }
}

async function runAssistant(action, extraArgs = []) {
  if (state.running) return;
  if (!state.projectPath) {
    appendLog("请先选择项目目录。");
    return;
  }
  localStorage.setItem("xiaxia.projectPath", state.projectPath);
  localStorage.setItem("xiaxia.commitMessage", state.commitMessage);
  setRunning(action, true);
  try {
    const result = await invoke("run_assistant", {
      projectPath: state.projectPath,
      action,
      extraArgs,
    });
    state.progress = result.code === 0 ? 100 : state.progress;
    appendLog(result.code === 0 ? "桌面端：执行完成" : `桌面端：执行失败，退出码 ${result.code}`);
  } catch (error) {
    appendLog(`桌面端错误：${error}`);
  } finally {
    setRunning("", false);
    await refreshStatus();
  }
}

async function runQuickCheck() {
  if (state.running) return;
  if (!state.projectPath) {
    appendLog("请先选择项目目录。");
    return;
  }
  localStorage.setItem("xiaxia.projectPath", state.projectPath);
  setRunning("quickCheck", true);
  try {
    const result = await invoke("quick_check", { projectPath: state.projectPath });
    state.logs = result.lines;
    state.progress = 100;
    appendLog(result.ok ? "桌面端：快速检查完成" : "桌面端：快速检查发现问题");
  } catch (error) {
    appendLog(`桌面端错误：${error}`);
  } finally {
    setRunning("", false);
    await refreshStatus();
  }
}

async function runNativeDeploy() {
  if (state.running) return;
  if (!state.projectPath) {
    appendLog("请先选择项目目录。");
    return;
  }
  const secrets = state.secretStatus;
  if (!secrets || !secrets.githubToken) {
    appendLog("请先在首次设置中配置 GitHub Token 后再发布。");
    return;
  }
  const message = state.commitMessage || `desktop deploy ${new Date().toLocaleString("zh-CN", { hour12: false })}`;
  localStorage.setItem("xiaxia.projectPath", state.projectPath);
  localStorage.setItem("xiaxia.commitMessage", message);
  state.commitMessage = message;
  setRunning("deploy", true);
  try {
    const result = await invoke("native_deploy", {
      projectPath: state.projectPath,
      message,
      githubToken: "",
      sshKey: "",
      serverPassword: "",
    });
    state.progress = result.code === 0 ? 100 : state.progress;
    appendLog(result.code === 0 ? "桌面端：原生发布完成" : `桌面端：原生发布失败，退出码 ${result.code}`);
  } catch (error) {
    appendLog(`桌面端错误：${error}`);
  } finally {
    setRunning("", false);
    await refreshStatus();
  }
}

async function saveSetupSecrets() {
  if (state.running) return;
  state.setupError = "";
  
  // 验证 GitHub Token 不为空
  if (!state.githubToken || state.githubToken.trim() === "") {
    state.setupError = "GitHub Token 不能为空";
    render();
    return;
  }
  
  try {
    state.secretStatus = await invoke("save_secret_config", {
      githubToken: state.githubToken,
      sshKey: "", // 不再需要用户输入
      serverPassword: "", // 不再需要用户输入
    });
    await invoke("complete_setup");
    state.githubToken = "";
    state.setupComplete = true;
    appendLog("OK  GitHub Token 已加密保存到本机，SSH 认证使用内置密钥");
    await refreshStatus();
  } catch (error) {
    state.setupError = `保存失败：${error}`;
    render();
  }
}

async function runNativeRollback(commit) {
  if (state.running) return;
  if (!state.projectPath) {
    appendLog("请先选择项目目录。");
    return;
  }
  localStorage.setItem("xiaxia.projectPath", state.projectPath);
  setRunning("rollback", true);
  try {
    const result = await invoke("native_rollback", {
      projectPath: state.projectPath,
      commit,
    });
    state.progress = result.code === 0 ? 100 : state.progress;
    appendLog(result.code === 0 ? "桌面端：原生回滚完成" : `桌面端：原生回滚失败，退出码 ${result.code}`);
  } catch (error) {
    appendLog(`桌面端错误：${error}`);
  } finally {
    setRunning("", false);
    await refreshStatus();
  }
}

function actionLabel() {
  if (!state.running) return "空闲";
  if (state.activeAction === "clone") return "正在初始化项目";
  if (state.activeAction === "quickCheck") return "正在快速检查";
  if (state.activeAction === "check") return "正在检查";
  if (state.activeAction === "deploy") return "正在发布";
  if (state.activeAction === "rollback") return "正在回滚";
  return "执行中";
}

function statusText() {
  if (!state.projectPath) return "未选择项目目录";
  if (!state.status) return "未读取";
  if (state.status.error) return state.status.error;
  const changed = state.status.status ? `${state.status.status.split("\n").length} 项变动` : "工作区干净";
  return `${state.status.branch || "未知分支"} · ${changed}`;
}

function secretStatusText() {
  const status = state.secretStatus;
  if (!status) return "未读取";
  if (status.githubToken) {
    return "已配置：GitHub Token ✓ | SSH 认证（内置）✓";
  }
  return "未配置 GitHub Token";
}

function renderSetupModal() {
  const errorBlock = state.setupError
    ? `<div class="setup-error">${escapeHtml(state.setupError)}</div>`
    : "";
  const hasSecrets = state.secretStatus && state.secretStatus.githubToken;
  const title = hasSecrets ? "修改 GitHub Token" : "首次设置";
  const desc = hasSecrets
    ? "修改已保存的 GitHub Token。"
    : "首次使用需要配置 GitHub Token 用于代码推送，SSH 私钥已内置无需配置。";
  return `
    <div class="setup-overlay">
      <div class="setup-modal">
        <h2>${title}</h2>
        <p>${desc}</p>
        ${errorBlock}
        <label>
          <span>GitHub Token <em>*</em></span>
          <input id="setupGithubToken" type="password" placeholder="用于 Git push/pull 的 GitHub Personal Access Token" value="${escapeHtml(state.githubToken)}" />
        </label>
        <p class="setup-hint">💡 提示：SSH 服务器认证已内置，无需配置密钥或密码。Token 将加密保存在本机。</p>
        <div class="setup-actions">
          <button id="saveSetup" class="primary">保存并开始使用</button>
        </div>
      </div>
    </div>
  `;
}

function renderHelpModal() {
  return `
    <div class="setup-overlay" id="helpOverlay">
      <div class="setup-modal help-modal">
        <div class="help-header">
          <h2>📖 使用说明</h2>
          <button id="closeHelp" class="close-btn" title="关闭">✕</button>
        </div>
        
        <div class="help-content">
          <section class="help-section">
            <h3>🚀 首次使用流程</h3>
            <ol>
              <li><strong>配置 GitHub Token</strong>：首次打开会要求输入 GitHub Token（用于代码推送）</li>
              <li><strong>初始化项目</strong>：如果没有项目，点击"初始化项目"从 GitHub 克隆</li>
              <li><strong>选择项目目录</strong>：如果已有项目，点击"浏览"选择项目文件夹</li>
              <li><strong>开始使用</strong>：配置完成后即可正常发布</li>
            </ol>
          </section>

          <section class="help-section">
            <h3>📝 日常发布流程</h3>
            <ol>
              <li><strong>修改代码</strong>：在项目中进行开发</li>
              <li><strong>刷新状态</strong>：查看修改了哪些文件</li>
              <li><strong>快速检查</strong>：确认项目状态正常（必需文件、Git、认证凭据等）</li>
              <li><strong>输入发布说明</strong>：描述本次修改内容</li>
              <li><strong>发布到服务器</strong>：一键提交、推送、打包、上传、部署</li>
            </ol>
          </section>

          <section class="help-section">
            <h3>🔍 检查功能说明</h3>
            <div class="help-item">
              <strong>刷新状态</strong>
              <p>更新当前显示的项目信息：分支、代码变动、最近 commit 等</p>
            </div>
            <div class="help-item">
              <strong>检查（快速检查）</strong>
              <p>本地快速验证：检查必需文件、SSH 认证、GitHub Token、Git 状态</p>
              <p>⏱️ 速度：快（几秒内完成）</p>
            </div>
            <div class="help-item">
              <strong>完整检查</strong>
              <p>深度检查：包括 JavaScript 语法、Python 导入、缓存使用等</p>
              <p>⏱️ 速度：较慢（需要 Python/Node.js 环境）</p>
            </div>
          </section>

          <section class="help-section">
            <h3>⏮️ 回滚操作</h3>
            <ol>
              <li>在"最近 5 次 commit"列表中找到要回滚的版本</li>
              <li>点击该版本右侧的"回滚"按钮</li>
              <li>服务器会自动恢复到该版本的代码并重启服务</li>
            </ol>
          </section>

          <section class="help-section">
            <h3>💡 状态指示器</h3>
            <div class="help-item">
              <strong>右上角状态</strong>
              <p>显示当前程序状态：空闲、正在检查、正在发布、正在回滚等</p>
            </div>
            <div class="help-item">
              <strong>进度条</strong>
              <p>显示当前操作的执行进度百分比</p>
            </div>
            <div class="help-item">
              <strong>执行日志</strong>
              <p>实时显示操作过程中的详细信息和错误提示</p>
            </div>
          </section>

          <section class="help-section">
            <h3>🔐 安全说明</h3>
            <ul>
              <li>GitHub Token 和服务器密码加密存储在本机</li>
              <li>SSH 私钥已内置在程序中，无需手动配置</li>
              <li>配置文件不会被 Git 提交或上传到网络</li>
              <li>只有本机用户可以访问这些凭据</li>
            </ul>
          </section>

          <section class="help-section">
            <h3>❓ 常见问题</h3>
            <div class="help-item">
              <strong>Q: 需要配置 SSH Key 吗？</strong>
              <p>A: 不需要！程序已内置服务器 SSH 私钥，开箱即用。</p>
            </div>
            <div class="help-item">
              <strong>Q: GitHub Token 在哪里获取？</strong>
              <p>A: GitHub → Settings → Developer settings → Personal access tokens → Generate new token</p>
            </div>
            <div class="help-item">
              <strong>Q: 发布失败怎么办？</strong>
              <p>A: 查看执行日志中的错误信息，常见原因：Git 冲突、网络问题、服务器连接失败</p>
            </div>
            <div class="help-item">
              <strong>Q: 可以在多台电脑使用吗？</strong>
              <p>A: 可以！每台电脑首次使用时都需要配置一次 GitHub Token。</p>
            </div>
          </section>
        </div>
      </div>
    </div>
  `;
}

function renderCommits() {
  if (!state.commits.length) return '<div class="empty">暂无 commit 信息</div>';
  return state.commits
    .map((commit, index) => {
      const disabled = state.running ? "disabled" : "";
      return `
        <div class="commit-row">
          <div class="commit-index">${index + 1}</div>
          <div class="commit-main">
            <div class="commit-title">${escapeHtml(commit.subject)}</div>
            <div class="commit-sha">${escapeHtml(commit.short)}</div>
          </div>
          <button class="small danger" data-rollback="${escapeHtml(commit.full)}" ${disabled}>回滚</button>
        </div>
      `;
    })
    .join("");
}

function renderStatusBlock(title, value, emptyText) {
  return `
    <section class="panel">
      <div class="panel-title">${title}</div>
      <pre class="status-pre">${value ? escapeHtml(value) : emptyText}</pre>
    </section>
  `;
}

function render() {
  // Still loading setup status
  if (state.setupComplete === null) {
    app.innerHTML = "";
    return;
  }

  // Show setup modal if not complete
  if (!state.setupComplete) {
    app.innerHTML = renderSetupModal();

    document.querySelector("#setupGithubToken")?.addEventListener("input", (event) => {
      state.githubToken = event.target.value.trim();
    });
    document.querySelector("#saveSetup")?.addEventListener("click", saveSetupSecrets);
    return;
  }

  // Main UI
  const status = state.status || {};
  app.innerHTML = `
    <main class="shell">
      <header class="topbar">
        <div>
          <h1>虾虾发布助手</h1>
          <p>${escapeHtml(statusText())}</p>
        </div>
        <div class="topbar-actions">
          <button id="showHelp" class="help-btn" title="使用说明">❓ 帮助</button>
          <div class="state-pill">${escapeHtml(actionLabel())}</div>
        </div>
      </header>

      <section class="control-band">
        <label>
          <span>项目目录</span>
          <div class="input-row">
            <input id="projectPath" placeholder="请点击浏览选择项目目录" value="${escapeHtml(state.projectPath)}" ${state.running ? "disabled" : ""} />
            <button id="browseProject" class="browse-btn" ${state.running ? "disabled" : ""} title="浏览文件夹">浏览</button>
          </div>
        </label>
        <label>
          <span>发布说明</span>
          <input id="commitMessage" placeholder="例如：修复案件查询分页" value="${escapeHtml(state.commitMessage)}" ${state.running ? "disabled" : ""} />
        </label>
        <div class="button-row">
          ${state.projectPath ? `
            <button id="refresh" ${state.running ? "disabled" : ""}>刷新状态</button>
            <button id="quickCheck" ${state.running ? "disabled" : ""}>检查</button>
            <button id="fullCheck" ${state.running ? "disabled" : ""}>完整检查</button>
            <button id="deploy" class="primary" ${state.running ? "disabled" : ""}>发布到服务器</button>
          ` : `
            <button id="initProject" class="primary" ${state.running ? "disabled" : ""}>初始化项目</button>
          `}
        </div>
      </section>

      <section class="secret-summary">
        <span>${escapeHtml(secretStatusText())}</span>
      </section>

      <section class="progress-panel">
        <div class="progress-head">
          <span>${escapeHtml(actionLabel())}</span>
          <strong>${state.progress}%</strong>
        </div>
        <div class="progress-track"><div style="width:${state.progress}%"></div></div>
      </section>

      <div class="grid">
        <section class="panel commits">
          <div class="panel-title">最近 5 次 commit</div>
          ${renderCommits()}
        </section>
        ${renderStatusBlock("代码变动", status.status, "当前没有本地代码变动")}
      </div>

      <div class="grid">
        ${renderStatusBlock("变动统计", status.diffStat, "暂无变动统计")}
        <section class="panel log-panel">
          <div class="panel-title">执行日志</div>
          <pre class="log-box">${state.logs.map(escapeHtml).join("\n") || "等待执行"}</pre>
        </section>
      </div>
    </main>
    ${state.showHelp ? renderHelpModal() : ""}
  `;

  document.querySelector("#projectPath")?.addEventListener("input", (event) => {
    state.projectPath = event.target.value.trim();
    localStorage.setItem("xiaxia.projectPath", state.projectPath);
  });
  document.querySelector("#browseProject")?.addEventListener("click", browseProjectPath);
  document.querySelector("#commitMessage")?.addEventListener("input", (event) => {
    state.commitMessage = event.target.value.trim();
  });
  document.querySelector("#refresh")?.addEventListener("click", refreshStatus);
  document.querySelector("#initProject")?.addEventListener("click", initProject);
  document.querySelector("#quickCheck")?.addEventListener("click", runQuickCheck);
  document.querySelector("#fullCheck")?.addEventListener("click", () => runAssistant("check"));
  document.querySelector("#deploy")?.addEventListener("click", runNativeDeploy);
  document.querySelectorAll("[data-rollback]").forEach((button) => {
    button.addEventListener("click", () => runNativeRollback(button.dataset.rollback));
  });
  document.querySelector("#showHelp")?.addEventListener("click", () => {
    state.showHelp = true;
    render();
  });
  document.querySelector("#closeHelp")?.addEventListener("click", () => {
    state.showHelp = false;
    render();
  });
  document.querySelector("#helpOverlay")?.addEventListener("click", (event) => {
    if (event.target.id === "helpOverlay") {
      state.showHelp = false;
      render();
    }
  });
}

listen("assistant-log", (event) => appendLog(event.payload.line));

render();
refreshStatus();
