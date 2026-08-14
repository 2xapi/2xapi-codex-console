// app.js — 2xapi Codex Console 前端（对齐 06 布局 + 04 契约）
"use strict";

// 供应商仅支持第三方接入（Mixed/PureApi）；官方登录不是供应商，由顶栏「⇄ 切官方」管理。
const MODES = [
  { key: "mixed", title: "保持官登 + API", sub: "Mixed · 走网关，保留官方登录" },
  { key: "pure_api", title: "第三方 API", sub: "PureApi · 纯第三方，覆盖 key" },
];
const MODE_LABEL = { official: "官方", mixed: "Mixed", pure_api: "PureApi" };

const state = {
  providers: [], activeId: null, health: null,
  selectedId: null, mode: "view", isNew: false, draft: null,
  preview: null, diag: null, fieldErrors: {}, toast: null,
  auth: null, showLogin: false, loginForm: { email: "", password: "" }, loginError: "",
  keyGroups: null, showKeyGroups: false,
  confirm: null,
  showTools: false, toolsTab: "launcher", backups: null, history: null,
  launcher: { useProvider: "", baseUrl: "", apiKey: "", model: "", projectDir: "", sessions: [], providers: null },
};

const $ = (s) => document.querySelector(s);
const esc = (s) => String(s == null ? "" : s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));

// ── 数据加载 ──
async function refreshAll() { await Promise.all([refreshProviders(), refreshHealth(), refreshSession()]); }
async function refreshSession() {
  try { const s = await api.session(); state.auth = s && s.authenticated ? s.user : null; }
  catch { state.auth = null; }
}
async function refreshProviders() {
  try {
    const d = await api.listProviders();
    state.providers = d.providers || [];
    state.activeId = d.active_provider_id || null;
  } catch (e) { showToast(e.message, "error"); }
}
async function refreshHealth() {
  try { state.health = await api.health(); } catch { state.health = null; }
}
function showToast(msg, kind = "info") {
  state.toast = { msg, kind };
  render();
  if (state.toast) setTimeout(() => { state.toast = null; render(); }, 2800);
}

// ── 渲染 ──
function render() { $("#app").innerHTML = shell(); }
function shell() {
  return topbar() +
    `<main class="layout"><section class="list-pane">${listPane()}</section><section class="detail-pane">${detailPane()}</section></main>` +
    toastEl() + loginModal() + keyGroupsModal() + confirmModal() + toolsModal();
}

function topbar() {
  const m = activeMode();
  const dotCls = m ? "dot " + m : "dot";
  const name = activeName();
  return `<header class="topbar">
    <span class="brand">2xapi Codex Console</span>
    <span class="spacer"></span>
    <span class="active-tag"><span class="${dotCls}"></span>当前：${esc(name)}${m ? " · " + MODE_LABEL[m] : ""}</span>
    ${state.auth ? `<button class="btn ghost" data-action="logout" type="button">登出 ${esc((state.auth.email || state.auth.name || "2xapi"))}</button>` : `<button class="btn ghost" data-action="show-login" type="button">登录 2xapi</button>`}
    <button class="btn ghost" data-action="show-tools" type="button">🛠 工具箱</button>
    <button class="btn ghost" data-action="activate-official" type="button">⇄ 切官方</button>
    <button class="btn ghost" data-action="refresh" type="button">↻</button>
  </header>`;
}
function activeName() {
  if (!state.activeId) return state.health && state.health.access_mode ? "官方" : "未激活";
  const p = state.providers.find((x) => x.id === state.activeId);
  return p ? p.name : "未激活";
}
function activeMode() { return (state.health && state.health.access_mode) || null; }

function listPane() {
  const items = state.providers.map((p) => {
    const isActive = p.id === state.activeId;
    const sel = p.id === state.selectedId ? "sel" : "";
    return `<div class="provider-item ${isActive ? "active" : ""} ${sel}" data-action="select" data-id="${esc(p.id)}">
      <span class="star">${isActive ? "★" : ""}</span>
      <span class="name">${esc(p.name)}</span>
      <span class="mode-tag ${esc(p.accessMode)}">${MODE_LABEL[p.accessMode] || p.accessMode}</span>
      <button class="icon-btn" data-action="delete" data-id="${esc(p.id)}" title="删除">✕</button>
    </div>`;
  }).join("");
  return `<h3>供应商</h3>${items || `<div class="empty">暂无供应商</div>`}<button class="btn primary" data-action="new" type="button" style="width:100%;margin-top:8px">+ 新建供应商</button>`;
}

function detailPane() {
  if (state.mode === "edit") return editForm();
  const p = state.providers.find((x) => x.id === state.selectedId);
  let html = detailView(p);
  if (state.preview) html += previewPanel(state.preview);
  if (state.diag && state.diag.id === state.selectedId) html += diagnosePanel();
  return html;
}

function detailView(p) {
  if (!p) return `<div class="empty">选择左侧供应商，或点「新建供应商」</div>`;
  const isActive = p.id === state.activeId;
  const row = (l, v) => `<div class="field full"><label>${l}</label><div>${esc(v == null || v === "" ? "—" : v)}</div></div>`;
  return `<div class="panel-card">
    <h2>${esc(p.name)} ${isActive ? '<span class="mode-tag mixed">已激活</span>' : ""}</h2>
    <div class="form-grid" style="margin-bottom:6px">
      ${row("接入模式", MODE_LABEL[p.accessMode])}
      ${row("协议", p.wireApi)}
      ${p.accessMode !== "official" ? row("上游地址", p.baseUrl) : ""}
      ${p.accessMode !== "official" ? row("API Key", p.apiKeyMasked) : ""}
      ${row("默认模型", p.model)}
      ${row("模型数", (p.models && p.models.length) || 0)}
    </div>
    <div class="btn-row">
      <button class="btn primary" data-action="activate" data-id="${esc(p.id)}">${isActive ? "重新激活" : "激活"}</button>
      <button class="btn" data-action="edit" data-id="${esc(p.id)}">编辑</button>
      <button class="btn" data-action="diagnose" data-id="${esc(p.id)}">诊断</button>
      <button class="btn" data-action="preview" data-id="${esc(p.id)}">预览 config</button>
    </div>
  </div>`;
}

// 后端 ModelConfig 是 snake_case，前端 draft 统一用 camelCase
function normModel(m) {
  return {
    name: m.name,
    displayName: m.display_name != null ? m.display_name : m.displayName,
    contextWindow: m.context_window != null ? m.context_window : m.contextWindow,
    isMultimodal: m.is_multimodal != null ? m.is_multimodal : m.isMultimodal,
    sendAsIs: m.send_as_is != null ? m.send_as_is : m.sendAsIs,
  };
}

function draftFromProvider(p) {
  return {
    id: p.id, name: p.name, accessMode: p.accessMode, baseUrl: p.baseUrl, apiKey: "",
    apiKeyMasked: p.apiKeyMasked, wireApi: p.wireApi, model: p.model,
    models: (p.models || []).map(normModel),
    proxyUrl: p.proxyUrl, timeoutSecs: p.timeoutSecs, userAgent: p.userAgent,
    websiteUrl: p.websiteUrl, notes: p.notes, icon: p.icon,
  };
}

function editForm() {
  const d = state.draft || {};
  const m = d.accessMode || "pure_api";
  const isOfficial = m === "official";
  const fe = state.fieldErrors || {};
  const ferr = (k) => (fe[k] ? `<div class="err-msg">${esc(fe[k])}</div>` : "");
  const fcls = (k) => "field " + (fe[k] ? "err" : "");
  const modeCards = MODES.map((mm) =>
    `<div class="mode-card ${mm.key === m ? "selected" : ""}" data-action="set-mode" data-mode="${mm.key}"><div class="t">${mm.title}</div><div class="s">${mm.sub}</div></div>`
  ).join("");

  const modelRows = (d.models || []).map((mdl, i) => `<tr>
    <td><input data-mk="name" data-i="${i}" value="${esc(mdl.name)}"></td>
    <td><input data-mk="displayName" data-i="${i}" value="${esc(mdl.displayName || "")}"></td>
    <td><input data-mk="contextWindow" data-i="${i}" value="${esc(mdl.contextWindow || "")}" style="width:84px"></td>
    <td style="text-align:center"><input type="checkbox" data-mk="isMultimodal" data-i="${i}" ${mdl.isMultimodal ? "checked" : ""}></td>
    <td style="text-align:center"><input type="checkbox" data-mk="sendAsIs" data-i="${i}" ${mdl.sendAsIs ? "checked" : ""}></td>
    <td><button class="icon-btn" data-action="del-model" data-i="${i}">✕</button></td>
  </tr>`).join("");

  return `<div class="panel-card">
    <h2>${state.isNew ? "新建供应商" : "编辑供应商"}</h2>
    <div class="mode-selector">${modeCards}</div>
    <div class="form-grid">
      <div class="${fcls("name")} full"><label>名称 *</label><input data-f="name" value="${esc(d.name || "")}">${ferr("name")}</div>
      ${isOfficial ? "" : `<div class="${fcls("baseUrl")} full"><label>上游地址 Base URL *</label><input data-f="baseUrl" placeholder="https://api.example.com" value="${esc(d.baseUrl || "")}">${ferr("baseUrl")}</div>`}
      ${isOfficial ? "" : `<div class="${fcls("apiKey")} full"><label>API Key ${state.isNew ? "*" : ""}</label><input data-f="apiKey" type="password" placeholder="${state.isNew ? "输入 Key" : (d.apiKeyMasked || "•••• 未改则留空")}" value="${state.isNew ? esc(d.apiKey || "") : ""}">${ferr("apiKey")}<span class="hint">编辑时留空表示不修改</span></div>`}
      ${(!isOfficial && state.auth) ? `<div class="full" style="grid-column:1/-1"><button class="btn ghost" data-action="show-keygroups" type="button">从 2xapi 账号导入 Key</button></div>` : ""}
      ${isOfficial ? "" : `<div class="${fcls("wireApi")}"><label>协议</label><select data-f="wireApi"><option value="responses" ${d.wireApi === "responses" ? "selected" : ""}>Responses</option><option value="chat_completions" ${d.wireApi === "chat_completions" ? "selected" : ""}>ChatCompletions</option></select></div>`}
      <div class="${fcls("model")}"><label>默认模型 *</label><input data-f="model" value="${esc(d.model || "")}">${ferr("model")}</div>
    </div>
    ${m === "pure_api" ? `<div class="notice">PureApi 将覆盖 auth.json 的 OPENAI_API_KEY；官方登录会自动备份（auth.json.official.bak），可一键「切官方」恢复。</div>` : ""}
    ${isOfficial ? "" : `<hr class="sep"><div class="muted" style="font-size:12px;margin-bottom:6px">模型列表</div>
      <table class="models-table"><thead><tr><th>模型名</th><th>显示名</th><th>上下文</th><th>多模态</th><th>透传</th><th></th></tr></thead><tbody>${modelRows}</tbody></table>
      <div class="btn-row"><button class="btn ghost" data-action="add-model" type="button">+ 模型行</button><button class="btn ghost" data-action="fetch-models" type="button">拉取模型</button></div>`}
    <details style="margin-top:10px"><summary class="muted" style="font-size:12px;cursor:pointer">高级（代理/超时/UA/官网/备注/图标）</summary>
      <div class="form-grid" style="margin-top:10px">
        <div class="field"><label>HTTP 代理</label><input data-f="proxyUrl" value="${esc(d.proxyUrl || "")}"></div>
        <div class="field"><label>超时(秒, 5~3600)</label><input data-f="timeoutSecs" type="number" value="${esc(d.timeoutSecs || "")}"></div>
        <div class="field"><label>User-Agent</label><input data-f="userAgent" value="${esc(d.userAgent || "")}"></div>
        <div class="field"><label>官网</label><input data-f="websiteUrl" value="${esc(d.websiteUrl || "")}"></div>
        <div class="field full"><label>备注</label><input data-f="notes" value="${esc(d.notes || "")}"></div>
        <div class="field"><label>图标(emoji)</label><input data-f="icon" value="${esc(d.icon || "")}"></div>
      </div>
    </details>
    <div class="btn-row">
      <button class="btn primary" data-action="save" type="button">保存</button>
      <button class="btn" data-action="preview-current" type="button">预览 config</button>
      <button class="btn ghost" data-action="cancel" type="button">取消</button>
    </div>
  </div>` + (state.preview ? previewPanel(state.preview) : "");
}

function previewPanel(pv) {
  const authLine = pv.auth_action === "set_key"
    ? `<div>auth.json：<strong>将设置 OPENAI_API_KEY</strong>　备份：<strong>${pv.backup_will_create ? "将创建 .official.bak" : "已存在备份，不覆盖"}</strong></div>`
    : `<div class="muted">auth.json：不变</div>`;
  return `<div class="panel-card"><h2>Config 预览（与实际写入一致）</h2><pre class="toml">${esc(pv.config_toml)}</pre>${authLine}</div>`;
}

function diagnosePanel() {
  const dg = state.diag;
  if (dg.loading) return `<div class="panel-card"><h2>Provider Doctor</h2><div class="muted">三步执行中…</div></div>`;
  const r = dg.result || {};
  const step = (label, ok, meta) => `<div class="step ${ok ? "ok" : "fail"}"><span class="icon">${ok ? "✓" : "✗"}</span><span class="label">${label}</span><span class="meta">${esc(meta || "")}</span></div>`;
  return `<div class="panel-card"><h2>Provider Doctor</h2>
    <div class="steps">
      ${step("Step1 配置校验", r.configValid, r.configValid ? "通过" : "未通过")}
      ${step("Step2 连接测试", r.reachable, r.reachable ? `延迟 ${r.latencyMs == null ? "—" : r.latencyMs + "ms"} · 模型 ${(r.models && r.models.length) || 0} 个` : "不可达")}
      ${step("Step3 真实请求", r.testOk, r.testOk ? "通过" : "未通过")}
    </div>
    ${r.errors && r.errors.length ? `<div class="notice">${r.errors.map((e) => esc("[" + e.step + "] " + e.message)).join("；")}</div>` : ""}
    <div class="btn-row"><button class="btn" data-action="diagnose" data-id="${esc(dg.id)}" type="button">重新诊断</button></div>
  </div>`;
}

function launcherPane() {
  const L = state.launcher;
  const manual = L.useProvider === "__manual__";
  const sel = (v) => (L.useProvider === v ? "selected" : "");
  const providerOpts =
    `<option value="" ${sel("")}>— 从软件 Provider 带入（key 用软件已填的）—</option>` +
    (L.providers || []).map((p) => `<option value="${esc(p.id)}" ${sel(p.id)}>${esc(p.name)}</option>`).join("") +
    `<option value="__manual__" ${sel("__manual__")}>手动填写（客户填自己的 key，单独计费）</option>`;
  const modelField = manual
    ? `<div class="field"><label>模型</label><input data-field="model" value="${esc(L.model)}" placeholder="gpt-5.6-sol"></div>`
    : `<div class="field"><label>模型（留空用 Provider 默认）</label><input data-field="model" value="${esc(L.model)}" placeholder="gpt-5.6-sol"></div>`;
  const manualFields = manual ? `
      <div class="field"><label>base_url</label><input data-field="baseUrl" value="${esc(L.baseUrl)}" placeholder="https://2xapi.cc.cd/v1"></div>
      <div class="field"><label>API Key</label><input type="password" data-field="apiKey" value="${esc(L.apiKey)}" placeholder="sk-..."></div>
      ${modelField}` : modelField;
  return `<div class="panel-card"><h2>🚀 Codex 启动器（直连版）</h2>
    <div class="form-grid">
      <div class="field full"><label>Key 来源</label>
        <select data-action="launcher-provider">${providerOpts}</select>
        <div class="hint">直连中转站端点、不开本地端口；独立 CODEX_HOME，不碰 ~/.codex；关闭 Codex 进程自动清理</div>
      </div>
      ${manualFields}
      <div class="field full"><label>项目目录</label><input data-field="projectDir" value="${esc(L.projectDir)}" placeholder="/Users/xxx/项目（必填）"></div>
    </div>
    <div class="btn-row">
      <button class="btn primary" data-action="launcher-start" type="button">▶ 使用（打开 Codex）</button>
      <button class="btn ghost" data-action="launcher-refresh" type="button">↻ 状态</button>
    </div>
    ${launcherSessionsHtml()}
  </div>`;
}

function launcherSessionsHtml() {
  const ss = state.launcher.sessions || [];
  if (!ss.length) return `<div class="muted" style="margin-top:12px">暂无运行中的启动会话</div>`;
  return `<div style="margin-top:12px"><h3 style="font-size:13px;margin:0 0 8px">启动会话</h3>
    ${ss.map((s) => `<div class="provider-item" style="cursor:default">
      <span class="name">${esc(s.model || "—")}</span>
      <span class="mode-tag ${s.alive ? "mixed" : ""}">${s.alive ? "运行中" : "已退出"}</span>
      <span class="muted" style="font-size:11px;flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(s.projectDir || "")}</span>
      ${s.alive ? `<button class="btn danger" data-action="launcher-stop" data-id="${esc(s.sessionId)}" type="button">停止</button>` : ""}
    </div>`).join("")}
  </div>`;
}

function loadLauncherStatus() {
  api.launcherStatus().then((r) => { state.launcher.sessions = (r && r.sessions) || []; render(); }).catch(() => {});
}

function doLauncherStart() {
  const L = state.launcher;
  const manual = L.useProvider === "__manual__";
  const val = (f) => { const el = document.querySelector(`[data-field="${f}"]`); return el ? el.value.trim() : ""; };
  const projectDir = val("projectDir");
  if (!projectDir) { showToast("请填写项目目录", "error"); return; }
  const body = { projectDir };
  if (manual) {
    const baseUrl = val("baseUrl"), apiKey = val("apiKey"), model = val("model");
    if (!baseUrl || !apiKey || !model) { showToast("手动模式需填 base_url / key / 模型", "error"); return; }
    body.baseUrl = baseUrl; body.apiKey = apiKey; body.model = model;
  } else {
    if (!L.useProvider) { showToast("请选择 Provider，或切到「手动填写」", "error"); return; }
    body.providerId = L.useProvider;
    const model = val("model");
    if (model) body.model = model;
  }
  api.launcherStart(body)
    .then((r) => { showToast("已启动 Codex，请查看系统终端窗口", "success"); loadLauncherStatus(); })
    .catch((e) => showToast(e.message, "error"));
}

function doLauncherStop(id) {
  api.launcherStop(id)
    .then(() => { showToast("已停止并清理临时目录", "success"); loadLauncherStatus(); })
    .catch((e) => showToast(e.message, "error"));
}

function toolsModal() {
  if (!state.showTools) return "";
  const tab = state.toolsTab;
  const tabBtn = (id, label) => `<button class="btn ${tab===id?"primary":"ghost"}" data-action="tools-tab" data-tab="${id}" type="button" style="flex:1">${label}</button>`;
  let body = "";
  if (tab === "launcher") {
    body = launcherPane();
  } else if (tab === "backups") {
    if (!state.backups) { body = `<div class="muted">加载中…</div>`; }
    else {
      const items = (state.backups.backups || []).map((b) => `<div class="provider-item" style="cursor:default">
        <span class="name">${esc(b.title || b.id || b.path || "备份")}</span>
        <span class="mode-tag">${esc(b.purpose || b.kind || "")}</span>
        <button class="btn ghost" data-action="restore-config" data-path="${esc(b.path)}" type="button">恢复</button>
      </div>`).join("");
      body = `${items || `<div class="muted">暂无备份</div>`}
        <div class="btn-row"><button class="btn primary" data-action="create-snapshot" type="button">创建快照</button></div>`;
    }
  } else if (tab === "history") {
    if (!state.history) { body = `<div class="muted">加载中…</div>`; }
    else {
      const s = state.history.state || {};
      body = `<div class="panel-card"><h2>历史会话诊断（只读）</h2>
        <div class="form-grid">
          <div class="field"><label>sessions</label><div>${s.total ?? "—"}</div></div>
          <div class="field"><label>rollouts</label><div>${s.rolloutTotal ?? "—"}</div></div>
        </div>
        <div class="notice" style="margin-top:10px">⚠️ 会话修复（SQLite 索引/rollout 对账）后端尚未实现，本期为只读诊断。后续开发补齐。</div>
      </div>`;
    }
  } else if (tab === "settings") {
    body = `<div class="panel-card"><h2>本机设置</h2>
      <div class="field full" style="margin:6px 0"><label>config.toml</label><div>${esc("/Users/" + "wenkezhi/.codex/config.toml")}</div></div>
      <div class="field full" style="margin:6px 0"><label>当前 Provider</label><div>${esc(state.activeId ? "custom（第三方）" : "openai（官方）")}</div></div>
      <div class="field full" style="margin:6px 0"><label>网关</label><div>127.0.0.1:8787（关窗不退出，托盘常驻）</div></div>
      <div class="field full" style="margin:6px 0"><label>写入策略</label><div>字段级合并（保留用户其他配置）；写入前自动备份</div></div>
    </div>`;
  }
  return `<div style="position:fixed;inset:0;background:rgba(0,0,0,.55);display:flex;align-items:center;justify-content:center;z-index:60" data-action="hide-tools">
    <div style="background:var(--panel);border:1px solid var(--border);border-radius:10px;padding:18px;width:560px;max-width:90vw;max-height:80vh;overflow:auto" onclick="event.stopPropagation()">
      <h2 style="margin:0 0 10px">工具箱</h2>
      <div style="display:flex;gap:6px;margin-bottom:14px">${tabBtn("launcher","🚀 启动器")} ${tabBtn("backups","备份恢复")} ${tabBtn("history","历史会话")} ${tabBtn("settings","本机设置")}</div>
      ${body}
      <div class="btn-row"><button class="btn ghost" data-action="hide-tools" type="button">关闭</button></div>
    </div></div>`;
}

function toastEl() { return state.toast ? `<div class="toast ${state.toast.kind}">${esc(state.toast.msg)}</div>` : ""; }

function confirmModal() {
  if (!state.confirm) return "";
  return `<div style="position:fixed;inset:0;background:rgba(0,0,0,.55);display:flex;align-items:center;justify-content:center;z-index:70" data-action="confirm-no">
    <div style="background:var(--panel);border:1px solid var(--border);border-radius:10px;padding:18px;width:340px;max-width:90vw">
      <div style="margin-bottom:16px">${esc(state.confirm.message)}</div>
      <div class="btn-row"><button class="btn danger" data-action="confirm-yes" type="button">删除</button><button class="btn ghost" data-action="confirm-no" type="button">取消</button></div>
    </div></div>`;
}
function askConfirm(message) {
  return new Promise((resolve) => { state.confirm = { message, resolve }; render(); });
}

function loginModal() {
  if (!state.showLogin) return "";
  const f = state.loginForm;
  return `<div style="position:fixed;inset:0;background:rgba(0,0,0,.55);display:flex;align-items:center;justify-content:center;z-index:60" data-action="hide-login">
    <div style="background:var(--panel);border:1px solid var(--border);border-radius:10px;padding:18px;width:360px;max-width:90vw">
      <h2 style="margin:0 0 4px">登录 2xapi 账号</h2>
      <div class="muted" style="font-size:12px;margin-bottom:10px">用于从 2xapi 获取 API Key（可选；直接填 Key 也可正常使用）</div>
      <div class="field" style="margin:8px 0"><label>邮箱</label><input data-login="email" value="${esc(f.email)}"></div>
      <div class="field" style="margin:8px 0"><label>密码</label><input data-login="password" type="password" value="${esc(f.password)}"></div>
      ${state.loginError ? `<div class="notice">${esc(state.loginError)}</div>` : ""}
      <div class="btn-row"><button class="btn primary" data-action="do-login" type="button">登录</button><button class="btn ghost" data-action="hide-login" type="button">取消</button></div>
    </div>
  </div>`;
}

function keyGroupsModal() {
  if (!state.showKeyGroups) return "";
  let body;
  if (!state.keyGroups) {
    body = `<div class="muted">加载中…</div>`;
  } else {
    const arr = Array.isArray(state.keyGroups) ? state.keyGroups : ((state.keyGroups && (state.keyGroups.groups || state.keyGroups.data)) || []);
    body = arr.length ? arr.map((g, i) => `<div class="provider-item" data-action="pick-keygroup" data-i="${i}"><span class="name">${esc(g.name || g.title || g.group || ("分组 " + i))}</span></div>`).join("") : `<div class="muted">没有可用分组</div>`;
  }
  return `<div style="position:fixed;inset:0;background:rgba(0,0,0,.55);display:flex;align-items:center;justify-content:center;z-index:60" data-action="hide-keygroups">
    <div style="background:var(--panel);border:1px solid var(--border);border-radius:10px;padding:18px;width:380px;max-width:90vw;max-height:80vh;overflow:auto">
      <h2 style="margin:0 0 10px">从 2xapi 选择 Key 分组</h2>${body}
      <div class="btn-row"><button class="btn ghost" data-action="hide-keygroups" type="button">关闭</button></div>
    </div>
  </div>`;
}

function collectLogin() {
  document.querySelectorAll("[data-login]").forEach((inp) => { state.loginForm[inp.dataset.login] = inp.value; });
}
function doLogin() {
  collectLogin();
  api.login(state.loginForm.email, state.loginForm.password)
    .then((r) => { state.auth = (r && r.user) || r || null; state.showLogin = false; state.loginError = ""; render(); showToast("已登录", "success"); })
    .catch((e) => { state.loginError = e.message; render(); });
}
function pickKeygroup(i) {
  const arr = Array.isArray(state.keyGroups) ? state.keyGroups : ((state.keyGroups && (state.keyGroups.groups || state.keyGroups.data)) || []);
  const g = arr[i] || {};
  collectDraft();
  const key = g.key || g.api_key || g.apiKey || g.token || "";
  const base = g.base_url || g.baseUrl || g.endpoint || "";
  if (key) state.draft.apiKey = key;
  if (base) state.draft.baseUrl = base;
  if (g.name || g.title) state.draft.name = g.name || g.title;
  state.showKeyGroups = false;
  render(); showToast("已导入，请检查并保存", "success");
}

// ── 表单收集 ──
function collectDraft() {
  if (!state.draft || !document.querySelector("[data-f]")) return;
  const d = state.draft;
  document.querySelectorAll("[data-f]").forEach((inp) => {
    const k = inp.dataset.f;
    if (k === "timeoutSecs") d[k] = inp.value ? Number(inp.value) : null;
    else d[k] = inp.value;
  });
  const models = [];
  document.querySelectorAll("tbody tr").forEach((tr) => {
    const pick = (mk) => tr.querySelector('[data-mk="' + mk + '"]');
    const name = pick("name");
    if (!name) return;
    const cw = pick("contextWindow");
    models.push({
      name: name.value || "",
      displayName: pick("displayName").value || undefined,
      contextWindow: cw && cw.value ? Number(cw.value) : undefined,
      isMultimodal: pick("isMultimodal").checked,
      sendAsIs: pick("sendAsIs").checked,
    });
  });
  d.models = models; // 不在此过滤空行（否则新增的空模型行会在下次重渲染被吞掉）；空行在保存时再过滤
}

// ── 动作 ──
async function doSave() {
  collectDraft();
  const d = state.draft;
  const body = {
    name: d.name, accessMode: d.accessMode, model: d.model,
    baseUrl: d.baseUrl || "", apiKey: d.apiKey || "", wireApi: d.wireApi || "responses",
    models: (d.models || []).filter((m) => m && m.name), proxyUrl: d.proxyUrl || "", timeoutSecs: d.timeoutSecs || null,
    userAgent: d.userAgent || "", websiteUrl: d.websiteUrl || "", notes: d.notes || "", icon: d.icon || "",
  };
  try {
    const saved = state.isNew ? await api.createProvider(body) : await api.updateProvider(d.id, body);
    state.fieldErrors = {}; state.preview = null;
    await refreshProviders();
    state.selectedId = saved.id; state.mode = "view"; state.isNew = false;
    showToast("已保存", "success");
  } catch (e) {
    state.fieldErrors = {};
    if (e.fields && e.fields.length) e.fields.forEach((f) => (state.fieldErrors[f] = "校验未通过（见顶部提示）"));
    showToast(e.message, "error");
    render();
  }
}

async function onClick(ev) {
  const t = ev.target.closest("[data-action]");
  if (!t) return;
  const a = t.dataset.action;
  const id = t.dataset.id;
  // 遮罩类动作（点遮罩关闭）：仅当点击元素就是遮罩本身时触发；
  // 点击弹窗内容区不得关闭（原为内联 stopPropagation，被 CSP 阻止后失效）。
  if (["confirm-no", "hide-login", "hide-keygroups"].includes(a) && ev.target !== t) return;
  ev.preventDefault();

  switch (a) {
    case "refresh": refreshAll().then(render); break;
    case "activate-official":
      api.activateOfficial().then((r) => refreshAll().then(() => { render(); showToast("已切官方" + (r.auth_restored ? " · 恢复了 auth.json" : ""), "success"); })).catch((e) => showToast(e.message, "error"));
      break;
    case "select":
      if (state.mode === "edit" && !await askConfirm("放弃未保存的修改？")) break; state.selectedId = id; state.mode = "view"; state.diag = null; state.preview = null; render(); break;
    case "new":
      if (state.mode === "edit" && !await askConfirm("放弃未保存的修改？")) break; state.draft = { accessMode: "pure_api", wireApi: "responses", models: [] }; state.isNew = true; state.mode = "edit"; state.fieldErrors = {}; state.preview = null; render(); break;
    case "edit": {
      const p = state.providers.find((x) => x.id === id); if (!p) break;
      state.draft = draftFromProvider(p); state.isNew = false; state.mode = "edit"; state.fieldErrors = {}; state.preview = null; render(); break;
    }
    case "set-mode": collectDraft(); state.draft.accessMode = t.dataset.mode; render(); break;
    case "add-model": collectDraft(); state.draft.models.push({ name: "", isMultimodal: false, sendAsIs: false }); render(); break;
    case "del-model": collectDraft(); state.draft.models.splice(Number(t.dataset.i), 1); render(); break;
    case "fetch-models": {
      collectDraft();
      const fmBody = state.draft.id ? { id: state.draft.id } : { baseUrl: state.draft.baseUrl, apiKey: state.draft.apiKey };
      api.fetchModels(fmBody).then((r) => {
        state.draft.models = (r.models || []).map(normModel);
        render();
        showToast("拉取到 " + (state.draft.models || []).length + " 个模型", "success");
        if (state.draft.id) refreshProviders();
      }).catch((e) => showToast(e.message, "error"));
      break;
    }
    case "save": doSave(); break;
    case "show-login": state.showLogin = true; state.loginError = ""; render(); break;
    case "hide-login": state.showLogin = false; render(); break;
    case "do-login": doLogin(); break;
    case "logout": api.logout().then(() => { state.auth = null; render(); showToast("已登出", "success"); }).catch((e) => showToast(e.message, "error")); break;
    case "show-keygroups": state.showKeyGroups = true; state.keyGroups = null; render(); api.keyGroups().then((g) => { state.keyGroups = g; render(); }).catch((e) => { state.showKeyGroups = false; showToast(e.message, "error"); render(); }); break;
    case "hide-keygroups": state.showKeyGroups = false; render(); break;
    case "pick-keygroup": pickKeygroup(Number(t.dataset.i)); break;
    case "confirm-yes": { const c = state.confirm; state.confirm = null; render(); if (c) c.resolve(true); break; }
    case "confirm-no": { const c = state.confirm; state.confirm = null; render(); if (c) c.resolve(false); break; }
    case "show-tools":
      state.showTools = true; state.toolsTab = "launcher"; state.backups = null; state.history = null; render();
      api.listProviders().then((d) => { state.launcher.providers = (d && d.providers) || (Array.isArray(d) ? d : []); render(); }).catch(() => {});
      api.launcherStatus().then((r) => { state.launcher.sessions = (r && r.sessions) || []; render(); }).catch(() => {});
      api.backups().then((b) => { state.backups = b; render(); }).catch((e) => showToast(e.message, "error"));
      api.inspectHistory().then((h) => { state.history = h; render(); }).catch(() => {});
      break;
    case "hide-tools": state.showTools = false; render(); break;
    case "tools-tab": state.toolsTab = t.dataset.tab; render(); break;
    case "launcher-provider": state.launcher.useProvider = t.value; state.launcher.model = ""; render(); break;
    case "launcher-start": doLauncherStart(); break;
    case "launcher-stop": doLauncherStop(t.dataset.id); break;
    case "launcher-refresh": loadLauncherStatus(); showToast("已刷新", "success"); break;
    case "create-snapshot": api.snapshot().then(() => showToast("快照已创建", "success")).then(() => api.backups()).then((b) => { state.backups = b; render(); }).catch((e) => showToast(e.message, "error")); break;
    case "restore-config":
      askConfirm("恢复此备份的 config.toml？").then((yes) => { if (!yes) return; api.restoreConfig(t.dataset.path).then(() => showToast("已恢复", "success")).catch((e) => showToast(e.message, "error")); });
      break;
    case "cancel": state.mode = "view"; state.draft = null; state.fieldErrors = {}; state.preview = null; render(); break;
    case "activate":
      api.activate(id).then((r) => refreshAll().then(() => { render(); showToast("已激活：config " + (r.config_written ? "已写" : "未变") + " · auth " + (r.auth_changed ? "已改" : "未动") + (r.backup_created ? " · 已备份" : ""), "success"); })).catch((e) => showToast(e.message, "error"));
      break;
    case "delete":
      ev.stopPropagation();
      if (!await askConfirm("删除该供应商？")) break;
      api.deleteProvider(id).then(() => { if (state.selectedId === id) { state.selectedId = null; state.mode = "view"; } return refreshAll(); }).then(render).then(() => showToast("已删除", "success")).catch((e) => showToast(e.message, "error"));
      break;
    case "preview":
      api.previewConfig({ id }).then((pv) => { state.preview = pv; render(); }).catch((e) => showToast(e.message, "error"));
      break;
    case "preview-current":
      collectDraft(); api.previewConfig(state.draft).then((pv) => { state.preview = pv; render(); }).catch((e) => showToast(e.message, "error"));
      break;
    case "diagnose":
      state.diag = { id, loading: true }; render();
      api.diagnose(id).then((r) => { state.diag = { id, loading: false, result: r }; render(); })
        .catch((e) => { state.diag = { id, loading: false, result: { configValid: false, reachable: false, latencyMs: null, models: [], testOk: false, errors: [{ step: "request", code: "E", message: e.message }] } }; render(); showToast(e.message, "error"); });
      break;
  }
}

// ── 启动 ──
function init() {
  document.addEventListener("click", onClick);
  document.addEventListener("change", (ev) => {
    const t = ev.target.closest("[data-action='launcher-provider']");
    if (!t) return;
    state.launcher.useProvider = t.value; state.launcher.model = ""; render();
  });
  refreshAll().then(render);
}
document.addEventListener("DOMContentLoaded", init);
