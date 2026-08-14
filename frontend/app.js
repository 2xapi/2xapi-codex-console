"use strict";
/* ── 2xapi Codex Console · 桌面版主通道形态(阶段 1,视觉规格 = 界面重设计原型) ──
 * 词汇规范:统一「供应商」;主动词不用于本卡(桌面版 = 开启托管/还原);「会话」仅指对话记录。
 * 交互规范:禁止内联 onclick(CSP),一律 data-a 事件委托;通路图断言 节点数=连线数+1。
 */

var state = {
  providers: [],        // GET /api/providers
  selId: null,          // 左栏/主卡当前选中供应商
  mode: "view",         // view | edit
  isNew: false,
  draft: null,          // 编辑草稿(input change 时收集,防重绘丢失)
  fieldErrors: {},
  diag: null,           // 诊断结果(当前选中供应商)
  dstate: null,         // GET /api/desktop/state {hasOfficial, hosting, gateway, codexHome}
  busy: null,           // 进行中动作标记(按钮禁用)
  modal: null,          // {kind:"login"|"snippet"|"tool", t:"history"|"settings"}
  toast: null,          // {m, k}
  confirmBox: null,     // {msg, resolve}
  loginError: "",
  session: null,        // 2xapi 登录态
};

var $ = function (s) { return document.querySelector(s); };
function esc(s) {
  return String(s == null ? "" : s).replace(/[&<>"]/g, function (c) {
    return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c];
  });
}
function showToast(m, k) {
  state.toast = { m: m, k: k || "" }; render();
  setTimeout(function () { state.toast = null; render(); }, 2800);
}
function askConfirm(msg) {
  return new Promise(function (resolve) { state.confirmBox = { msg: msg, resolve: resolve }; render(); });
}
function lineOf(id) { return state.providers.find(function (p) { return p.id === id; }) || null; }
function hosting() { return (state.dstate && state.dstate.hosting) || null; }
function hostedBy(id) { var h = hosting(); return !!h && h.way === "gateway" && (id ? h.providerId === id : true); }
var CHIP_COLORS = ["var(--c-gw)", "var(--c-direct)", "var(--c-accel)", "var(--c-official)"];
function chipColor(p, i) { return p.iconColor || CHIP_COLORS[i % CHIP_COLORS.length]; }

// ── 数据加载 ──
function normProviders(d) {
  var arr = (d && d.providers) || (Array.isArray(d) ? d : []);
  return arr.filter(function (p) { return p && p.accessMode !== "official"; });
}
async function refreshProviders() {
  var d = await api.listProviders();
  state.providers = normProviders(d);
  if (state.providers.length && !lineOf(state.selId)) {
    var h = hosting();
    state.selId = (h && h.providerId && lineOf(h.providerId)) ? h.providerId : state.providers[0].id;
  }
}
async function refreshDesktop() {
  try { state.dstate = await api.desktopState(); } catch (e) { state.dstate = null; }
}
async function refreshSession() {
  try { state.session = await api.session(); } catch (e) { state.session = null; }
}
async function refreshAll() {
  await Promise.all([refreshProviders(), refreshDesktop(), refreshSession()]);
}

// ── 渲染 ──
function render() {
  $("#app").innerHTML =
    '<header class="topbar">'
    + '<span class="brand"><span class="mark">2×</span>2xapi Codex Console</span>'
    + '<span class="spacer"></span>'
    + topChips()
    + loginBtn()
    + '</header>'
    + '<div class="frame"><nav class="rail">' + rail() + '</nav><main class="content">' + mainPane() + '</main></div>'
    + '<div class="foot-note">desktop-channel · 桌面版主通道 · 终端注入式方案已保存备用</div>'
    + (state.modal ? modalHtml() : "")
    + (state.confirmBox ? confirmHtml() : "")
    + (state.toast ? '<div class="toast ' + state.toast.k + '">' + esc(state.toast.m) + '</div>' : "");
  assertRouteShape();
}

/* 通路图形状自检:节点数 = 连线数 + 1(原型踩过的坑) */
function assertRouteShape() {
  var st = document.querySelectorAll("#app .route > .st").length;
  var lk = document.querySelectorAll("#app .route > .lk").length;
  if (st !== lk + 1) console.warn("通路图形状异常: 节点 " + st + " ≠ 连线 " + lk + " + 1");
}

function topChips() {
  var gw = (state.dstate && state.dstate.gateway) || null;
  var h = hosting();
  var hasOff = state.dstate && state.dstate.hasOfficial;
  var label;
  if (h && h.way === "gateway") {
    label = "桌面版:已托管 · " + (hasOff ? "混入" : "纯API") + " · " + esc(h.providerName || (lineOf(h.providerId) || {}).name || "");
  } else if (hasOff) {
    label = "桌面版:官方";
  } else {
    label = "桌面版:未配置";
  }
  return '<span class="gw-chip ' + (gw && gw.alive ? "alive" : "") + '"><span class="led"></span>gateway ' + (gw ? esc(gw.addr) : "127.0.0.1:8787") + '</span>'
    + '<span class="gw-chip ' + (h && h.way === "gateway" ? "on" : "") + '">' + label + '</span>';
}
function loginBtn() {
  var s = state.session;
  var logged = !!(s && (s.loggedIn || s.email));
  return logged
    ? '<button class="btn ghost" data-a="logout">' + esc(s.email || "已登录") + " · 登出</button>"
    : '<button class="btn ghost" data-a="login">登录 2xapi</button>';
}

function rail() {
  var rows = state.providers.map(function (p, i) {
    var isHost = hostedBy(p.id);
    return '<button class="line-row ' + (p.id === state.selId ? "sel" : "") + '" style="--lc:' + chipColor(p, i) + '" data-a="sel" data-id="' + esc(p.id) + '">'
      + '<span class="chip">' + esc(p.icon || String(i + 1)) + '</span><span class="nm">' + esc(p.name) + '</span>'
      + (isHost
        ? '<span class="tag" style="border-color:var(--c-gw);color:var(--c-gw)">托管中</span>'
        : '<span class="tag">' + (p.wireApi === "chat_completions" ? "chat" : "responses") + '</span>')
      + '</button>';
  }).join("");
  return '<div class="eyebrow">供应商</div>' + (rows || '<div class="sub" style="margin:4px 8px 10px">还没有供应商,点下方新建</div>')
    + '<button class="btn ghost new" data-a="new">＋ 新建供应商</button>'
    + '<div class="rail-foot"><a data-a="tool" data-t="history">历史会话</a><a data-a="tool" data-t="settings">本机设置</a></div>';
}

/* ── 桌面版主卡:账号状态自动检测 × 通路(本期 gateway;direct 待字段实测,加速待阶段 4) ── */
function desktopCard() {
  var d = state.dstate || {};
  var hasOff = !!d.hasOfficial;
  var h = hosting();
  var isHost = !!(h && h.way === "gateway");
  var p = lineOf(state.selId) || lineOf(h && h.providerId);
  var modeName = hasOff ? "混入模式" : "纯 API 模式";
  var acctSub = hasOff ? "官方登录保留" : "纯 API · 无官方账号";

  var st = function (c, b, s) { return '<div class="st" style="--lc:' + c + '"><span class="dot"></span><span class="lb"><b>' + b + '</b><span>' + s + '</span></span></div>'; };
  var lk = function (c, live) { return '<div class="lk ' + (live ? "live" : "") + '" style="--lc:' + c + '"></div>'; };

  var route, note, mech;
  if (!isHost) {
    route = st("var(--c-official)", "桌面版 Codex", hasOff ? "官方登录" : "未配置")
      + lk("var(--c-official)", false)
      + st("var(--c-official)", hasOff ? "官方 OpenAI" : "不可用", hasOff ? "chatgpt 登录" : "无官方登录");
    note = '<div class="route-mode"><span class="k" style="color:var(--c-official)">●</span>'
      + (hasOff ? "当前:官方直连 · 点下方按钮开启走中转" : "当前:未配置(无官方登录,官方通道不可用)· 开启托管后走中转") + '</div>';
    mech = hasOff
      ? '<span>官方登录 · 官方额度</span><span>未做任何修改</span>'
      : '<span>无官方登录</span><span>未做任何修改</span>';
  } else {
    route = st("var(--c-gw)", "桌面版 Codex", acctSub)
      + lk("var(--c-gw)", true)
      + st("var(--c-gw)", "网关", "127.0.0.1:8787")
      + lk("var(--c-gw)", true)
      + st(p ? chipColor(p, state.providers.indexOf(p)) : "var(--c-gw)", esc(p ? p.name : "?"), "中转站");
    note = '<div class="route-mode"><span class="k">●</span>通路二:网关(加速即将上线,当前直发上游) · 配置文件零 Key,Key 由网关注入 · ' + modeName + '</div>';
    mech = (hasOff ? '<span>① 官方登录/插件保留</span>' : '<span>① 无需官方账号</span>')
      + '<span>② 配置文件零 Key</span><span>③ 协议转换 · chat 中转可用</span><span>依赖本 app 常驻</span>';
  }

  var opts = state.providers.map(function (x) {
    return '<option value="' + esc(x.id) + '"' + (x.id === state.selId ? " selected" : "") + '>' + esc(x.name) + (x.model ? "(" + esc(x.model) + ")" : "") + '</option>';
  }).join("");
  var hostPid = isHost ? (h.providerId || state.selId) : state.selId;

  return '<section class="card"><h2>桌面版 Codex(ChatGPT.app)· 主通道</h2>'
    + '<div class="sub">一键走中转;账号状态自动检测(<b>有官方账号 → 混入模式,登录保留</b>;无账号 → 纯 API 模式),全程自动备份、一键还原。</div>'
    + '<div style="display:flex;align-items:center;gap:8px;margin:0 0 10px">'
    + '<span class="tag" style="' + (hasOff ? "border-color:var(--c-official);color:var(--c-official)" : "") + '">检测:官方登录 ' + (hasOff ? "✓" : "未检出") + ' → ' + modeName + '</span>'
    + '</div>'
    + '<div class="route">' + route + '</div>'
    + note
    + '<div class="mech">' + mech + '</div>'
    + '<div class="grid">'
    + '<div class="f full"><label>通路方式</label><div class="seg">'
    + '<button data-a="way" data-w="direct" aria-pressed="false" style="--lc:var(--c-direct)" disabled>直连 API 端点<small>即将支持 · 不依赖本 app</small></button>'
    + '<button data-a="way" data-w="gateway" aria-pressed="true" style="--lc:var(--c-gw)"' + (isHost ? "" : " disabled") + '>网关(推荐)<small>零落盘 · 加速可开关</small></button>'
    + '</div>'
    + (isHost
      ? '<div class="seg" style="margin-top:8px;max-width:460px" title="阶段 4 上线">'
        + '<button data-a="accel" data-m="off" aria-pressed="true" style="--lc:var(--muted)" disabled>加速:关<small>网关直发上游</small></button>'
        + '<button data-a="accel" data-m="official" aria-pressed="false" style="--lc:var(--c-accel)" disabled>官方加速专线<span class="badge-soon">即将上线</span><small>2xapi 站专用</small></button>'
        + '<button data-a="accel" data-m="custom" aria-pressed="false" style="--lc:var(--c-accel)" disabled>我的节点<span class="badge-soon">即将上线</span><small>自己的 VPS / 本地代理</small></button>'
        + '</div>'
      : "")
    + '</div>'
    + '<div class="f"><label>供应商(走哪家中转)</label><select data-a="lsel"' + (state.providers.length ? "" : " disabled") + '>'
    + (opts || '<option value="">先新建供应商</option>') + '</select><div class="hint">切换即时生效,无需重启桌面版</div></div>'
    + '<div class="f"><label>状态</label><div style="padding:9px 0;font-size:13px">'
    + (isHost
      ? '<span class="tag" style="border-color:var(--c-gw);color:var(--c-gw)">已托管 · ' + modeName + '</span> <span class="hint">网关 · 配置零 Key</span>'
      : '<span class="tag">' + (hasOff ? "未托管 · 走官方" : "未托管 · 未配置") + '</span>')
    + '</div></div>'
    + '</div>'
    + '<div class="btn-row" style="margin-top:14px">'
    + (isHost
      ? '<button class="btn" data-a="unhost"' + (state.busy === "unhost" ? " disabled" : "") + '>' + (hasOff ? "还原官方" : "关闭托管(移除中转配置)") + (state.busy === "unhost" ? "…" : "") + '</button>'
      : '<button class="btn primary" data-a="host"' + (!hostPid || state.busy === "host" ? " disabled" : "") + ' style="--lc:var(--c-gw)">开启:桌面版走中转' + (state.busy === "host" ? "…" : "") + '</button>')
    + '<button class="btn ghost" data-a="test">⚡ 测试连接</button>'
    + '</div>'
    + '<div id="rtest"></div>'
    + '</section>';
}

/* ── CLI 注入式:方案已保存,本版不启用 ── */
function cliBackupCard() {
  return '<section class="card"><details>'
    + '<summary>终端注入式启动器(Codex CLI)· 方案已保存,本版暂不启用</summary>'
    + '<div class="sub" style="margin-top:10px">点一下即开终端版 Codex:零写入、多供应商并行、可同时开多个。完整设计见方案 v3(-c 参数覆盖 / 真 home / 密钥即焚),界面与代码均已备,后续按需启用。</div>'
    + '<button class="btn" disabled>▶ 启动 Codex(终端版)· 暂未启用</button>'
    + '</details></section>';
}

/* ── 供应商详情 / 编辑 ── */
function detailCard() {
  if (state.mode === "edit") return editCard();
  var p = lineOf(state.selId);
  if (!p) return "";
  var kv = function (k, v, mono) {
    return '<div><div class="k">' + k + '</div><div class="v ' + (mono ? "mono" : "") + '">' + esc(v == null || v === "" ? "—" : v) + '</div></div>';
  };
  var isHost = hostedBy(p.id);
  var html = '<section class="card"><div class="eyebrow" style="margin:0 0 8px">供应商详情 · ' + esc(p.name) + (isHost ? ' <span class="tag" style="border-color:var(--c-gw);color:var(--c-gw)">桌面版托管中</span>' : "") + '</div>'
    + '<div class="kv">'
    + kv("上游地址", p.baseUrl, true)
    + kv("API Key", p.apiKeyMasked || "—", true)
    + kv("协议", p.wireApi === "chat_completions" ? "chat(自动经网关转换)" : "responses", true)
    + kv("默认模型", p.model, true)
    + kv("模型数", (p.models || []).length || "—", true)
    + kv("备注", p.notes)
    + "</div>"
    + '<div class="btn-row">'
    + (isHost ? '<button class="btn" disabled>✓ 桌面版正在使用</button>' : '<button class="btn primary" data-a="use-line">▶ 桌面版改用这条线</button>')
    + '<button class="btn" data-a="edit">编辑</button>'
    + '<button class="btn" data-a="diag">' + (state.diag ? "收起诊断" : "诊断") + '</button>'
    + '<button class="btn ghost" data-a="snippet">复制 config 片段(进阶)</button>'
    + '<button class="btn ghost danger" data-a="del">删除</button>'
    + '</div>'
    + '<div class="sub" style="margin:10px 0 0">「复制片段」给愿意手动配置的用户;普通用户点上方按钮即可,一切自动。</div>'
    + "</section>";
  if (state.diag && state.diag.forId === p.id) html += diagCard(state.diag.data);
  return html;
}

function diagCard(d) {
  if (!d) {
    // 诊断进行中(diagnose 含真实网络请求,可达数秒):占位而非空卡
    return '<section class="card"><div class="eyebrow" style="margin:0 0 10px">诊断 / doctor</div><div class="steps" style="margin-top:0">'
      + '<div class="step">⟳ 诊断进行中…<span class="meta">连接测试 + 真实请求</span></div></div></section>';
  }
  var ok = function (b) { return b ? "✓" : "✗"; };
  var cls = function (b) { return b ? "" : " bad"; };
  var errs = (d.errors || []).map(function (e) { return esc(e.message || e.msg || String(e)); }).join(";");
  return '<section class="card"><div class="eyebrow" style="margin:0 0 10px">诊断 / doctor</div><div class="steps" style="margin-top:0">'
    + '<div class="step' + cls(d.configValid) + '">' + ok(d.configValid) + ' 配置校验<span class="meta">' + (d.configValid ? "pass" : "fail") + '</span></div>'
    + '<div class="step' + cls(d.reachable) + '">' + ok(d.reachable) + ' 连接测试<span class="meta">' + (d.reachable ? ((d.latencyMs != null ? d.latencyMs + "ms · " : "") + (d.models || []).length + " models") : "不通") + '</span></div>'
    + '<div class="step' + cls(d.testOk) + '">' + ok(d.testOk) + ' 真实请求<span class="meta">' + (d.testOk ? "pass" : "fail") + '</span></div>'
    + '</div>'
    + (errs ? '<div class="notice">' + errs + '</div>' : "")
    + '</section>';
}

function editCard() {
  var d = state.draft;
  var fe = function (f) { return state.fieldErrors[f] ? '<div class="err">' + esc(state.fieldErrors[f]) + '</div>' : ""; };
  var fc = function (f) { return state.fieldErrors[f] ? " has-err" : ""; };
  var rows = (d.models || []).map(function (x, i) {
    return '<tr><td><input data-mf="name" data-mi="' + i + '" value="' + esc(x.name || "") + '"></td>'
      + '<td><input data-mf="cw" data-mi="' + i + '" style="width:90px" value="' + esc(x.contextWindow || "") + '"></td>'
      + '<td><button class="btn ghost danger" data-a="mrow-del" data-i="' + i + '">✕</button></td></tr>';
  }).join("");
  var wireSel = d.wireApi === "chat_completions" ? "chat_completions" : "responses";
  return '<section class="card"><h2>' + (state.isNew ? "新建供应商" : "编辑供应商 · " + esc(d.name)) + '</h2>'
    + '<div class="sub">填好地址和 Key,点「拉取模型」自动获取模型列表;Key 只存在本软件里,不写入任何配置文件。</div>'
    + '<div class="grid">'
    + '<div class="f full' + fc("name") + '"><label>名称 *</label><input data-f="name" value="' + esc(d.name || "") + '">' + fe("name") + '</div>'
    + '<div class="f full' + fc("baseUrl") + '"><label>上游地址 *</label><input class="mono" data-f="baseUrl" value="' + esc(d.baseUrl || "") + '" placeholder="https://api.example.com">' + fe("baseUrl") + '</div>'
    + '<div class="f full' + fc("apiKey") + '"><label>api key' + (state.isNew ? " *" : " · 留空不修改") + '</label><input type="password" class="mono" data-f="apiKey" placeholder="' + (state.isNew ? "sk-..." : (d.apiKeyMasked ? "•••• 未改则留空" : "sk-...")) + '" value="">' + fe("apiKey") + '</div>'
    + '<div class="f' + fc("model") + '"><label>默认模型 *</label><input class="mono" data-f="model" value="' + esc(d.model || "") + '" placeholder="点「拉取模型」后自动填入">' + fe("model") + '</div>'
    + "</div>"
    + '<div class="eyebrow" style="margin:16px 0 6px">模型列表(「拉取模型」自动填写,一般无需手改)</div>'
    + '<table class="mtable"><thead><tr><th>模型名</th><th>上下文</th><th></th></tr></thead><tbody>' + rows + '</tbody></table>'
    + '<div class="btn-row">'
    + '<button class="btn ghost" data-a="mfetch"' + (state.busy === "mfetch" ? " disabled" : '') + '>' + (state.busy === "mfetch" ? "拉取中…" : "⤓ 拉取模型") + '</button>'
    + '<button class="btn ghost" data-a="mrow-add">＋ 手动加一行</button>'
    + '</div>'
    + '<details style="margin-top:10px"><summary>高级(协议 · 代理 · 超时 · 备注)· 不用动</summary><div class="grid" style="margin-top:10px">'
    + '<div class="f"><label>协议</label><select data-f="wireSel"><option value="auto"' + (d.wireSelUi !== wireSel ? " selected" : "") + '>自动(拉取模型时检测)</option><option value="responses"' + (d.wireSelUi === "responses" ? " selected" : "") + '>Responses</option><option value="chat_completions"' + (d.wireSelUi === "chat_completions" ? " selected" : "") + '>ChatCompletions</option></select><div class="hint">不确定就保持「自动」</div></div>'
    + '<div class="f"><label>HTTP 代理</label><input class="mono" data-f="proxyUrl" value="' + esc(d.proxyUrl || "") + '" placeholder="http://127.0.0.1:7890"></div>'
    + '<div class="f"><label>超时(秒)</label><input type="number" data-f="timeoutSecs" value="' + esc(d.timeoutSecs || "") + '"></div>'
    + '<div class="f full"><label>备注</label><input data-f="notes" value="' + esc(d.notes || "") + '"></div>'
    + '</div></details>'
    + '<div class="btn-row" style="margin-top:16px">'
    + '<button class="btn primary" data-a="save"' + (state.busy === "save" ? " disabled" : "") + '>保存</button>'
    + '<button class="btn ghost" data-a="cancel">取消</button>'
    + '</div>'
    + "</section>";
}

function mainPane() { return desktopCard() + cliBackupCard() + detailCard(); }

/* ── 弹窗 ── */
function confirmHtml() {
  return '<div class="mask" style="z-index:70" data-a="cno"><div class="box" style="width:330px"><div style="margin-bottom:16px">' + esc(state.confirmBox.msg) + '</div><div class="btn-row" style="margin:0"><button class="btn danger" data-a="cyes">删除</button><button class="btn ghost" data-a="cno">取消</button></div></div></div>';
}
function modalHtml() {
  var m = state.modal;
  if (m.kind === "login") {
    return '<div class="mask" data-a="mclose"><div class="box" style="width:350px"><h2 style="margin:0 0 4px;font-size:15px">登录 2xapi 账号</h2>'
      + '<div class="sub">登录后可一键导入你的 Key 和供应商</div>'
      + '<div class="f" style="margin:8px 0"><label>邮箱</label><input data-l="email" value="' + esc(state.loginEmail || "") + '"></div>'
      + '<div class="f" style="margin:8px 0"><label>密码</label><input type="password" data-l="password"></div>'
      + (state.loginError ? '<div class="err" style="color:var(--c-err);font-size:12px">' + esc(state.loginError) + '</div>' : "")
      + '<div class="btn-row"><button class="btn primary" data-a="do-login">登录</button><button class="btn ghost" data-a="mclose">取消</button></div></div></div>';
  }
  if (m.kind === "snippet") {
    return '<div class="mask" data-a="mclose"><div class="box"><h2 style="margin:0 0 4px;font-size:15px">config 片段(进阶,可选)</h2>'
      + '<div class="sub">仅给想手动配置 ~/.codex 的用户:自行粘贴、自行负责。日常使用点「开启:桌面版走中转」即可,无需任何手动配置。</div>'
      + '<pre class="toml">model_provider = "custom"\n\n[model_providers.custom]\nname = "custom"\nbase_url = "http://127.0.0.1:8787"\nwire_api = "responses"\nrequires_openai_auth = true</pre>'
      + '<div class="btn-row"><button class="btn primary" data-a="copy-snippet">复制到剪贴板</button><button class="btn ghost" data-a="mclose">关闭</button></div></div></div>';
  }
  var body;
  if (m.t === "history") {
    body = '<div class="notice">历史会话管理即将上线(阶段 3):统一列表、按供应商筛选、一键修复。</div>';
  } else {
    body = '<div class="kv"><div><div class="k">config.toml</div><div class="v mono">~/.codex(托管开启时仅一处 custom 段,零 Key)</div></div><div><div class="k">网关</div><div class="v mono">127.0.0.1:8787 · 托盘常驻</div></div></div>'
      + '<div class="f full" style="margin-top:12px"><label>我的加速节点(即将上线 · 仅本机保存)</label><input class="mono" placeholder="socks5://127.0.0.1:7890 或 http://用户:密码@你的VPS:8443" disabled><div class="hint">阶段 4 上线:自己的 VPS(跑 gost/squid)或本地代理客户端端口。官方加速专线由 2xapi 下发,无需填写。</div></div>';
  }
  var title = { history: "历史会话", settings: "本机设置" }[m.t];
  return '<div class="mask" data-a="mclose"><div class="box"><h2 style="margin:0 0 12px;font-size:15px">' + title + '</h2>' + body + '<div class="btn-row"><button class="btn ghost" data-a="mclose">关闭</button></div></div></div>';
}

/* ── 草稿收集(重绘前从 DOM 收值,防丢输入) ── */
function collectDraft() {
  if (state.mode !== "edit" || !state.draft) return;
  var d = state.draft;
  var get = function (f) { var el = document.querySelector('[data-f="' + f + '"]'); return el ? el.value : undefined; };
  ["name", "baseUrl", "model", "proxyUrl", "timeoutSecs", "notes"].forEach(function (f) {
    var v = get(f);
    if (v !== undefined) d[f] = v.trim ? v.trim() : v;
  });
  // apiKey 特殊:输入框不回显(重绘后 value 恒空),空 = 未输入 → 保留草稿值,不覆盖
  var ak = get("apiKey");
  if (typeof ak === "string" && ak !== "") d.apiKey = ak;
  var wsel = get("wireSel");
  if (wsel !== undefined) d.wireSelUi = wsel; // "auto" = 保持现值(落库 wireApi 不变);显式选了才更新
  var mnames = document.querySelectorAll('[data-mf="name"]');
  mnames.forEach(function (el) { d.models[Number(el.dataset.mi)].name = el.value.trim(); });
  document.querySelectorAll('[data-mf="cw"]').forEach(function (el) {
    var v = el.value.trim();
    d.models[Number(el.dataset.mi)].contextWindow = v ? Number(v) : null;
  });
}

function draftFromProvider(p) {
  return {
    id: p.id, name: p.name, baseUrl: p.baseUrl || "", apiKey: "", apiKeyMasked: p.apiKeyMasked || "",
    wireApi: p.wireApi || "responses", wireSelUi: p.wireApi || "responses",
    model: p.model || "", models: (p.models || []).map(function (m) { return { name: m.name, contextWindow: m.contextWindow }; }),
    proxyUrl: p.proxyUrl || "", timeoutSecs: p.timeoutSecs || "", notes: p.notes || "",
  };
}

function normModel(m) {
  return {
    name: m.name || m.id || m.model || "",
    contextWindow: m.contextWindow || m.context_window || null,
  };
}

/* ── 动作 ── */
async function doHost(providerId) {
  state.busy = "host"; render();
  try {
    var r = await api.desktopHost(providerId, "gateway");
    await refreshAll();
    state.selId = providerId;
    showToast(r.switched ? "已切换供应商(即时生效)" : "桌面版已托管走中转(已自动备份,可随时还原)", "ok");
  } catch (e) {
    if (e.code === "E_DIRECT_UNAVAILABLE") showToast("直连方式即将支持,当前请使用网关方式", "error");
    else showToast(e.message, "error");
    await refreshDesktop();
  }
  state.busy = null; render();
}
async function doUnhost() {
  if (!await askConfirm("还原桌面版配置?托管写入的中转设置将被移除,官方登录/手写配置不受影响。")) return;
  state.busy = "unhost"; render();
  try {
    var r = await api.desktopUnhost();
    await refreshAll();
    showToast(r.restored ? "已还原(可从备份目录恢复)" : "当前未托管,无需还原", "ok");
  } catch (e) { showToast(e.message, "error"); await refreshDesktop(); }
  state.busy = null; render();
}

async function doSave() {
  collectDraft();
  var d = state.draft;
  var errs = {};
  if (!d.name) errs.name = "必填";
  if (!d.baseUrl) errs.baseUrl = "必填";
  if (state.isNew && !d.apiKey) errs.apiKey = "新建必填";
  if (!d.model) errs.model = "必填(可先点「拉取模型」)";
  state.fieldErrors = errs;
  if (Object.keys(errs).length) { render(); showToast("还有必填项未完成", "error"); return; }
  var body = {
    name: d.name, accessMode: "pure_api", model: d.model,
    baseUrl: d.baseUrl, apiKey: d.apiKey || "",
    wireApi: d.wireSelUi === "auto" ? d.wireApi : d.wireSelUi,  // 「自动」= 保持现值
    models: (d.models || []).filter(function (m) { return m && m.name; }),
    proxyUrl: d.proxyUrl || "", timeoutSecs: d.timeoutSecs ? Number(d.timeoutSecs) : null,
    notes: d.notes || "",
  };
  state.busy = "save"; render();
  try {
    var saved = state.isNew ? await api.createProvider(body) : await api.updateProvider(d.id, body);
    state.fieldErrors = {};
    await refreshProviders();
    state.selId = saved.id; state.mode = "view"; state.isNew = false; state.draft = null;
    showToast("供应商已保存(仅存于本软件,未写任何配置)", "ok");
  } catch (e) {
    state.fieldErrors = {};
    if (e.fields && e.fields.length) e.fields.forEach(function (f) { state.fieldErrors[f] = "校验未通过"; });
    showToast(e.message, "error");
  }
  state.busy = null; render();
}

async function doFetchModels() {
  collectDraft();
  var d = state.draft;
  var fmBody = (!state.isNew && d.id) ? { id: d.id } : { baseUrl: d.baseUrl, apiKey: d.apiKey };
  if (!state.isNew && d.apiKey) fmBody = { id: d.id, apiKey: d.apiKey }; // 编辑时改了 key → 用显式 key 探测
  if (fmBody.baseUrl !== undefined && (!fmBody.baseUrl || !fmBody.apiKey)) {
    showToast("新建供应商请先填上游地址和 api key", "error"); return;
  }
  state.busy = "mfetch"; render();
  try {
    var r = await api.fetchModels(fmBody);
    d.models = (r.models || []).map(normModel);
    if (!d.model && d.models.length) d.model = d.models[0].name; // 自动默认第一个,降低小白负担
    state.busy = null; render();
    showToast("拉取到 " + d.models.length + " 个模型" + (d.models.length ? ",默认模型已填入" : ""), "ok");
  } catch (e) {
    state.busy = null; render();
    showToast("拉取模型失败:" + e.message, "error");
  }
}

async function doDiag() {
  var p = lineOf(state.selId);
  if (!p) return;
  if (state.diag && state.diag.forId === p.id) { state.diag = null; render(); return; }
  state.diag = { forId: p.id, data: null }; render();
  try {
    var d = await api.diagnose(p.id);
    state.diag = { forId: p.id, data: d };
  } catch (e) { state.diag = null; showToast(e.message, "error"); }
  render();
}

async function doDelete() {
  var p = lineOf(state.selId);
  if (!p) return;
  if (!await askConfirm('删除供应商「' + p.name + '」?此操作只删本软件记录,不动任何配置文件。')) return;
  try {
    await api.deleteProvider(p.id);
    if (hostedBy(p.id)) await refreshDesktop(); // active 被清,托管态可能变化
    await refreshProviders();
    showToast("已删除", "ok");
  } catch (e) { showToast(e.message, "error"); }
  render();
}

async function doLogin() {
  var email = (document.querySelector('[data-l="email"]') || {}).value || "";
  var password = (document.querySelector('[data-l="password"]') || {}).value || "";
  if (!email || !password) { state.loginError = "邮箱和密码都要填"; render(); return; }
  try {
    await api.login(email, password);
    state.modal = null; state.loginError = "";
    await refreshSession();
    showToast("登录成功", "ok");
  } catch (e) {
    state.loginError = e.message; render();
  }
}

/* ── 事件(委托,无内联处理器) ── */
document.addEventListener("click", function (ev) {
  var t = ev.target.closest("[data-a]");
  if (!t) return;
  var a = t.dataset.a;
  if (a === "cno" && ev.target !== t) return;   // 点弹窗内容不关
  if (a === "mclose" && ev.target !== t) return;
  ev.preventDefault();
  switch (a) {
    case "sel": collectDraft(); state.selId = t.dataset.id; state.mode = "view"; state.diag = null; render(); break;
    case "new":
      state.draft = { name: "", baseUrl: "", apiKey: "", model: "", models: [], wireApi: "responses", wireSelUi: "responses", proxyUrl: "", timeoutSecs: "", notes: "" };
      state.isNew = true; state.mode = "edit"; state.fieldErrors = {}; state.diag = null; render(); break;
    case "edit": {
      var p = lineOf(state.selId); if (!p) break;
      state.draft = draftFromProvider(p); state.isNew = false; state.mode = "edit"; state.fieldErrors = {}; state.diag = null; render(); break;
    }
    case "cancel": state.mode = "view"; state.draft = null; state.fieldErrors = {}; render(); break;
    case "save": doSave(); break;
    case "mfetch": doFetchModels(); break;
    case "mrow-add": collectDraft(); state.draft.models.push({ name: "", contextWindow: null }); render(); break;
    case "mrow-del": collectDraft(); state.draft.models.splice(Number(t.dataset.i), 1); render(); break;
    case "use-line": if (state.selId) doHost(state.selId); break;
    case "host": { var pid = (hosting() && hosting().providerId) || state.selId; if (pid) doHost(pid); break; }
    case "unhost": doUnhost(); break;
    case "lsel": break; // change 事件处理
    case "diag": doDiag(); break;
    case "del": doDelete(); break;
    case "snippet": state.modal = { kind: "snippet" }; render(); break;
    case "copy-snippet": {
      var txt = document.querySelector(".box pre.toml");
      if (txt && navigator.clipboard) navigator.clipboard.writeText(txt.textContent).then(function () { showToast("片段已复制(进阶自担;日常请用一键托管)", "ok"); });
      break;
    }
    case "login": state.loginError = ""; state.modal = { kind: "login" }; render(); break;
    case "do-login": doLogin(); break;
    case "logout": api.logout().then(function () { state.session = null; render(); showToast("已登出", "ok"); }).catch(function (e) { showToast(e.message, "error"); }); break;
    case "tool": state.modal = { kind: "tool", t: t.dataset.t }; render(); break;
    case "mclose": state.modal = null; render(); break;
    case "cyes": { var c = state.confirmBox; state.confirmBox = null; render(); if (c) c.resolve(true); break; }
    case "cno": { var c2 = state.confirmBox; state.confirmBox = null; render(); if (c2) c2.resolve(false); break; }
    case "test": showToast("测试连接即将上线(下一阶段)", "ok"); break;
  }
});

/* 下拉/change:输入收集 + 供应商切换(已托管 = 热切换) */
document.addEventListener("change", function (ev) {
  var sel = ev.target.closest("[data-a='lsel']");
  if (sel) {
    var id = sel.value;
    if (hosting() && hosting().way === "gateway" && id !== hosting().providerId) {
      state.selId = id; render(); doHost(id); // 已托管:切下拉 = 切中转(网关热切换)
    } else {
      collectDraft(); state.selId = id; render();
    }
    return;
  }
  if (ev.target.closest("[data-f], [data-mf], [data-l]")) collectDraft();
});
/* 输入中也收集(避免重绘时机丢半个字) */
document.addEventListener("input", function (ev) {
  if (ev.target.closest("[data-f], [data-mf], [data-l]")) collectDraft();
});

/* ── 启动 ── */
refreshAll().then(render).catch(function (e) { console.error(e); render(); });
