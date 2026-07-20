import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./styles.css";

const DEFAULT_PROJECT = "/Users/chance/liuk";

const state = {
  projectPath: localStorage.getItem("xiaxia.projectPath") || DEFAULT_PROJECT,
  commitMessage: localStorage.getItem("xiaxia.commitMessage") || "",
  githubToken: "",
  sshKey: "",
  serverPassword: "",
  secretStatus: null,
  running: false,
  activeAction: "",
  progress: 0,
  logs: [],
  status: null,
  commits: [],
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
    state.status = await invoke("project_status", { projectPath: state.projectPath });
    state.commits = await invoke("recent_commits", { projectPath: state.projectPath });
    state.secretStatus = await invoke("saved_secrets_status");
  } catch (error) {
    state.status = { error: String(error), branch: "", status: "", diffStat: "" };
  }
  render();
}

async function runAssistant(action, extraArgs = []) {
  if (state.running) return;
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
  const message = state.commitMessage || `desktop deploy ${new Date().toLocaleString("zh-CN", { hour12: false })}`;
  localStorage.setItem("xiaxia.projectPath", state.projectPath);
  localStorage.setItem("xiaxia.commitMessage", message);
  state.commitMessage = message;
  setRunning("deploy", true);
  try {
    const result = await invoke("native_deploy", {
      projectPath: state.projectPath,
      message,
      githubToken: state.githubToken,
      sshKey: state.sshKey,
      serverPassword: state.serverPassword,
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

async function saveSecrets() {
  if (state.running) return;
  try {
    state.secretStatus = await invoke("save_secret_config", {
      githubToken: state.githubToken,
      sshKey: state.sshKey,
      serverPassword: state.serverPassword,
    });
    state.githubToken = "";
    state.sshKey = "";
    state.serverPassword = "";
    appendLog("OK  免输入配置已保存到本机加密文件");
  } catch (error) {
    appendLog(`保存失败：${error}`);
  }
}

async function runNativeRollback(commit) {
  if (state.running) return;
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
  if (state.activeAction === "quickCheck") return "正在快速检查";
  if (state.activeAction === "check") return "正在检查";
  if (state.activeAction === "deploy") return "正在发布";
  if (state.activeAction === "rollback") return "正在回滚";
  return "执行中";
}

function statusText() {
  if (!state.status) return "未读取";
  if (state.status.error) return state.status.error;
  const changed = state.status.status ? `${state.status.status.split("\n").length} 项变动` : "工作区干净";
  return `${state.status.branch || "未知分支"} · ${changed}`;
}

function secretStatusText() {
  const status = state.secretStatus;
  if (!status) return "未读取";
  const items = [];
  if (status.githubToken) items.push("GitHub Token");
  if (status.sshKey) items.push("SSH 私钥路径");
  if (status.serverPassword) items.push("服务器密码");
  return items.length ? `已保存：${items.join("、")}` : "未保存免输入配置";
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
  const status = state.status || {};
  app.innerHTML = `
    <main class="shell">
      <header class="topbar">
        <div>
          <h1>虾虾发布助手</h1>
          <p>${escapeHtml(statusText())}</p>
        </div>
        <div class="state-pill">${escapeHtml(actionLabel())}</div>
      </header>

      <section class="control-band">
        <label>
          <span>项目目录</span>
          <input id="projectPath" value="${escapeHtml(state.projectPath)}" ${state.running ? "disabled" : ""} />
        </label>
        <label>
          <span>发布说明</span>
          <input id="commitMessage" placeholder="例如：修复案件查询分页" value="${escapeHtml(state.commitMessage)}" ${state.running ? "disabled" : ""} />
        </label>
        <label>
          <span>GitHub Token</span>
          <input id="githubToken" type="password" placeholder="留空则用已保存 Token" value="${escapeHtml(state.githubToken)}" ${state.running ? "disabled" : ""} />
        </label>
        <div class="button-row">
          <button id="refresh" ${state.running ? "disabled" : ""}>刷新状态</button>
          <button id="quickCheck" ${state.running ? "disabled" : ""}>检查</button>
          <button id="fullCheck" ${state.running ? "disabled" : ""}>完整检查</button>
          <button id="deploy" class="primary" ${state.running ? "disabled" : ""}>发布到服务器</button>
        </div>
      </section>

      <section class="secret-band">
        <label>
          <span>SSH 私钥路径</span>
          <input id="sshKey" placeholder="留空则用配置文件或已保存路径" value="${escapeHtml(state.sshKey)}" ${state.running ? "disabled" : ""} />
        </label>
        <label>
          <span>服务器密码</span>
          <input id="serverPassword" type="password" placeholder="可留空，优先使用 SSH 私钥" value="${escapeHtml(state.serverPassword)}" ${state.running ? "disabled" : ""} />
        </label>
        <div class="secret-actions">
          <button id="saveSecrets" ${state.running ? "disabled" : ""}>保存免输入配置</button>
          <span>${escapeHtml(secretStatusText())}</span>
        </div>
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
  `;

  document.querySelector("#projectPath")?.addEventListener("input", (event) => {
    state.projectPath = event.target.value.trim();
  });
  document.querySelector("#commitMessage")?.addEventListener("input", (event) => {
    state.commitMessage = event.target.value.trim();
  });
  document.querySelector("#githubToken")?.addEventListener("input", (event) => {
    state.githubToken = event.target.value.trim();
  });
  document.querySelector("#sshKey")?.addEventListener("input", (event) => {
    state.sshKey = event.target.value.trim();
  });
  document.querySelector("#serverPassword")?.addEventListener("input", (event) => {
    state.serverPassword = event.target.value;
  });
  document.querySelector("#refresh")?.addEventListener("click", refreshStatus);
  document.querySelector("#saveSecrets")?.addEventListener("click", saveSecrets);
  document.querySelector("#quickCheck")?.addEventListener("click", runQuickCheck);
  document.querySelector("#fullCheck")?.addEventListener("click", () => runAssistant("check"));
  document.querySelector("#deploy")?.addEventListener("click", runNativeDeploy);
  document.querySelectorAll("[data-rollback]").forEach((button) => {
    button.addEventListener("click", () => runNativeRollback(button.dataset.rollback));
  });
}

listen("assistant-log", (event) => appendLog(event.payload.line));

render();
refreshStatus();
