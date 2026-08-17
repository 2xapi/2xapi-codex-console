"use strict";
/* ── 2xapi Codex Console · 界面重构 v2(视觉 = 无滚动条布局演示,逐块对齐;数据 = api-client 接真) ──
 * 词汇规范:统一「供应商」;主动词「开启托管 / 还原官方」;「会话」仅作名词;加速 seg「关 / 开·自动择优」。
 * 交互规范:禁内联 onclick(CSP),一律 data-a 事件委托;通路图形状自检 节点数=连线数+1。
 */

var state = {
  agent: "codex",      // codex | claude
  view: "dash",        // dash | history
  selId: null,         // 当前选中供应商
  providers: [],       // GET /api/providers(pure_api 过滤后;含 agent 字段,按 agent 分流)
  dstate: null,        // GET /api/desktop/state {hasOfficial, gateway, hosting}(Codex 托管)
  claude: null,        // Claude 注入式(前端本地态:null 或 {started, way, providerId, providerName, env, command, model};后端无 claude-state 接口)
  hermes: null,        // Hermes 托管态(GET /api/desktop/hermes/state:{hosting:{way,entry}|null, pointer, configPath})
  codexWay: "gateway", // Codex 通路方式(会话内本地态 gateway|direct;direct 由 hasOfficial===false 门控,不落盘)
  accel: null,         // GET /api/accel/state {mode, customNode, lines, scopeNote, usage}
  session: null,       // GET /api/session
  balance: null,       // GET /api/auth/me → user.balance
  menuOpen: false,     // 账号菜单展开
  search: "",          // 供应商栏筛选
  setTab: "ip",        // 设置五分区
  sessions: null,      // GET /api/sessions items
  sessionsTotal: 0,
  sessionsSettings: null,
  sessionsRepairing: false,
  claudeSessions: null,      // GET /api/claude/sessions items(null=未加载/加载中;只读展示,无修复/删除)
  claudeSessionsTotal: 0,
  claudeSessionsPage: 0,     // 已加载到第几页(50/页)
  claudeSessionsLoading: false,
  nodeDraft: null,     // IP 管理「我的代理」输入草稿(重绘防丢)
  nodeTest: null,      // {busy} | {ok,latencyMs} | {err,msg}
  importKeys: null,    // {keys, baseUrl}
  importBusy: false,
  edit: null,          // 编辑草稿 {id,isNew,name,baseUrl,apiKey,model,wireApi,models}
  fieldErrors: {},
  test: null,          // 测试连接结果
  diag: null,          // 诊断结果 {forId, data}
  busy: null,          // 进行中动作
  loginEmail: "", loginPassword: "", loginError: "", remembered: false,
  balShow: true,       // 顶栏实时余额开关(localStorage 持久)
  confirmCb: null,
  toastTimer: null,
};

var $ = function (s) { return document.querySelector(s); };
function esc(s) {
  return String(s == null ? "" : s).replace(/[&<>"]/g, function (c) {
    return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c];
  });
}
var CHIP_COLORS = ["var(--c-gw)", "var(--c-direct)", "var(--c-accel)", "var(--c-official)"];
function chipColor(p, i) { return p.iconColor || CHIP_COLORS[i % CHIP_COLORS.length]; }
/* 当前 agent 的供应商列表(listProviders 返回全部,前端按 agent 字段分流;旧数据缺省 codex) */
function providersFor(agent) {
  return state.providers.filter(function (p) { return (p.agent || "codex") === agent; });
}
function lineOf(id) { return providersFor(state.agent).find(function (p) { return p.id === id; }) || null; }
function hosting() {
  var d = state.dstate;
  var h = d && d.hosting;
  return (h && (h.way === "gateway" || h.way === "direct")) ? h : null;
}
function claudeStarted() { var c = state.claude; return !!(c && c.started); }
function hermesHosted() { var h = state.hermes; return !!(h && h.hosting); }
function hermesPointerName() { var h = state.hermes; return (h && h.pointer) || ""; }
function claudeWay() { var c = state.claude; return (c && c.way) || "gateway"; }
function codexWayNow() {
  var w = state.codexWay || "gateway";
  /* 直连仅在 hasOfficial === false(纯 API 模式)可用;官方登录在/状态未知 → 回落网关 */
  if (w === "direct" && !(state.dstate && state.dstate.hasOfficial === false)) return "gateway";
  return w;
}
function hostedBy(id) {
  if (state.agent === "claude") { var c = state.claude; return !!(c && c.started && c.providerId === id); }
  if (state.agent === "hermes") return false; /* hermes 托管为整体态(条目存在),不绑死单个供应商 */
  var h = hosting(); return !!h && h.providerId === id;
}
function wireLabel(p) {
  var w = p.wireApi;
  if (w === "chat_completions") return "chat";
  if (w === "anthropic") return "anthropic";
  return w || "responses";
}
function loggedIn() {
  var s = state.session;
  return !!(s && (s.authenticated || s.loggedIn || s.email || (s.user && s.user.email)));
}
function sessionEmail() {
  var s = state.session || {};
  return (s.user && s.user.email) || s.email || "";
}
function fmtTime(ms) {
  if (!ms) return "—";
  var d = new Date(ms);
  var pad = function (n) { return n < 10 ? "0" + n : "" + n; };
  var today = new Date();
  var sameDay = d.getFullYear() === today.getFullYear() && d.getMonth() === today.getMonth() && d.getDate() === today.getDate();
  var hh = pad(d.getHours()) + ":" + pad(d.getMinutes());
  if (sameDay) return "今天 " + hh;
  return (d.getMonth() + 1) + "-" + pad(d.getDate()) + " " + hh;
}
function normModel(m) {
  return { name: m.name || m.id || m.model || "", contextWindow: m.contextWindow || m.context_window || null };
}

/* ── toast / confirm(页面内反馈,替换原生 alert/confirm)── */
function showToast(msg, kind) {
  var root = document.getElementById("toastRoot"); if (!root) return;
  root.innerHTML = '<div class="toast ' + (kind || "") + '">' + esc(msg) + '</div>';
  clearTimeout(state.toastTimer);
  state.toastTimer = setTimeout(function () { root.innerHTML = ""; }, 2600);
}
function askConfirm(title, msg) {
  return new Promise(function (resolve) {
    document.getElementById("cfTitle").textContent = title;
    document.getElementById("cfMsg").textContent = msg;
    state.confirmCb = resolve;
    document.getElementById("confirmMask").style.display = "";
  });
}
function closeConfirm() {
  document.getElementById("confirmMask").style.display = "none";
  state.confirmCb = null;
}

/* ── 数据加载 ── */
function normProviders(d) {
  var arr = (d && d.providers) || (Array.isArray(d) ? d : []);
  return arr.filter(function (p) { return p && p.accessMode !== "official"; });
}
async function refreshProviders() {
  var d = await api.listProviders();
  state.providers = normProviders(d);
  var mine = providersFor(state.agent);
  if (mine.length && !lineOf(state.selId)) {
    var h = hosting();
    state.selId = (h && h.providerId && lineOf(h.providerId)) ? h.providerId : mine[0].id;
  }
}
async function refreshDesktop() {
  try { state.dstate = await api.desktopState(); } catch (e) { state.dstate = null; }
  /* 已托管的实际方式回写 seg 态(如上次会话直连托管,本会话 seg 对齐事实) */
  var h = state.dstate && state.dstate.hosting;
  if (h && (h.way === "direct" || h.way === "gateway")) state.codexWay = h.way;
}
async function refreshClaudeState() {
  /* 后端无 claude-state 接口:注入态纯前端本地;注入的供应商若已被删除 → 复位未注入 */
  var c = state.claude;
  if (c && c.started) {
    var p = providersFor("claude").find(function (x) { return x.id === c.providerId; });
    if (!p) state.claude = null;
  }
}
async function refreshHermesState() {
  /* Hermes 托管态:{hosting:{way,entry}|null, pointer, configPath}(叠加条目存在性 = 托管标记) */
  try { state.hermes = await api.agentState("hermes"); } catch (e) { state.hermes = null; }
}
async function refreshAccel() {
  try {
    state.accel = await api.accelState();
  } catch (e) {
    state.accel = state.accel || { mode: "off", customNode: "", lines: [], scopeNote: "", usage: { ok: false, degradedToDirect: false } };
  }
}
async function refreshSession() {
  try { state.session = await api.session(); } catch (e) { state.session = null; }
  state.balance = null;
  if (loggedIn()) {
    try {
      var me = await api.me();
      var u = (me && me.user) || me || {}; // 兼容 {user:{balance}} 与顶层 {balance}
      if (typeof u.balance === "number") state.balance = u.balance;
    } catch (e) { /* 下次刷新再试 */ }
  }
}
/* 超融合 A 线一期:能力标签(供应商×模型粒度;manual 覆盖不冲) */
var CAP_TAGS = null;
function refreshCapTags() {
  return api.capabilityTags().then(function (v) { CAP_TAGS = (v && v.tags) || {}; })
    .catch(function () { CAP_TAGS = {}; });
}
function capsEntry(p) {
  return (CAP_TAGS && CAP_TAGS[p.id + "::" + (p.model || "")]) || null;
}
function capsSectionHtml(p) {
  var e = capsEntry(p);
  var dims = [["text", "文本"], ["tools", "工具"], ["reasoning", "推理"], ["image_in", "识图"]];
  var chips = "";
  dims.forEach(function (d) {
    var val = e && e.caps ? (e.caps[d[0]] || "unknown") : "unknown";
    var cls = val === "yes" ? "tag on" : "tag";
    var lb = d[1] + (val === "yes" ? " ✓" : val === "no" ? " ✗" : " ?");
    chips += '<span class="' + cls + '">' + lb + '</span>';
  });
  var manualTag = e && e.source === "manual" ? ' <span class="tag">手动</span>' : '';
  var selects = dims.map(function (d) {
    return '<span style="display:inline-flex;align-items:center;gap:3px;margin-right:8px">' + d[1]
      + '<select data-a="cap-set" data-id="' + esc(p.id) + '" data-model="' + esc(p.model || "") + '" data-dim="' + d[0] + '">'
      + '<option value="">auto</option><option value="on">on</option><option value="off">off</option>'
      + '</select></span>';
  }).join("");
  return '<div style="margin:10px 0 0;padding:10px 12px;background:rgba(120,160,255,.05);border:1px solid rgba(120,160,255,.22);border-radius:9px">'
    + '<div style="display:flex;align-items:center;justify-content:space-between;gap:8px;flex-wrap:wrap">'
    + '<span style="font-size:11.5px;color:#A8C0F0">能力(模型 ' + esc(p.model || "未设置") + ')' + manualTag + ' ' + chips + '</span>'
    + '<button class="btn ghost" data-a="cap-probe" data-id="' + esc(p.id) + '"' + (state.busy === "cap-probe" ? " disabled" : "") + '>⚡ 探测能力</button></div>'
    + '<div style="margin-top:6px;font-size:10.5px;color:var(--muted)">覆盖:' + selects + '<span>auto=按探测;on/off=手动兜底(探测不冲);重探强制回 auto</span></div>'
    + '</div>';
}
function doCapProbe(id) {
  state.busy = "cap-probe"; render();
  api.probeCapabilities(id, { force: true }).then(function () {
    return refreshCapTags();
  }).then(function () {
    showToast("能力探测完成", "ok");
  }).catch(function (e2) {
    showToast(e2.message || "探测失败", "error");
  }).finally(function () {
    state.busy = null; render();
  });
}
function doCapSet(id, model, dim, val) {
  if (!val) return; // auto 空值=不清除选择,由重探恢复
  api.capabilitySet({ providerId: id, model: model, dim: dim, val: val }).then(function () {
    return refreshCapTags();
  }).then(function () {
    showToast(dim + " 已覆盖为 " + val, "ok");
  }).catch(function (e) {
    showToast(e.message || "覆盖失败", "error");
  }).finally(function () { render(); });
}

async function refreshAll() {
  await Promise.all([refreshProviders(), refreshDesktop(), refreshSession(), refreshAccel(), refreshClaudeState(), refreshCapTags()]);
}

/* ── 渲染 ── */
function renderNav() {
  var noRail = state.view === "history" || state.view === "eco" || state.view === "plug";
  document.getElementById("frame").classList.toggle("no-rail", noRail);
  document.querySelectorAll(".nav-btn.agent").forEach(function (b) {
    b.classList.toggle("on", b.dataset.g === state.agent);
  });
  var hb = document.getElementById("nv-history");
  if (hb) hb.classList.toggle("on", state.view === "history");
  var eb = document.getElementById("nv-eco");
  if (eb) eb.classList.toggle("on", state.view === "eco");
  var pb = document.getElementById("nv-plug");
  if (pb) pb.classList.toggle("on", state.view === "plug");
  /* 网关 chip:地址 + 存活灯 */
  var gw = (state.dstate && state.dstate.gateway) || null;
  var chip = document.getElementById("gwChip");
  if (chip) {
    var led = chip.querySelector(".led");
    chip.lastChild.textContent = " " + ((gw && gw.addr) || "127.0.0.1:8787");
    if (led) led.classList.toggle("off", !(gw && gw.alive));
  }
  /* 托管 chip:Codex 走桌面托管,Claude 走注入式 */
  var h = hosting();
  var hc = document.getElementById("hostChip");
  if (hc) {
    hc.classList.toggle("claude", state.agent === "claude");
    if (state.agent === "claude") {
      var c = state.claude;
      hc.textContent = (c && c.started && c.providerName) ? "Claude · 注入 " + c.providerName : "Claude · 未注入";
    } else {
      hc.textContent = (h && h.providerName) ? "托管 · " + h.providerName : "未托管";
    }
  }
}
function renderRail() {
  var el = document.getElementById("railList"); if (!el) return;
  var isC = state.agent === "claude";
  var who = isC ? "Claude" : "Codex";
  var mine = providersFor(state.agent);
  if (!mine.length) {
    el.innerHTML =
      '<div class="rail-head"><span class="eyebrow">' + who + ' 供应商</span><span class="tag">0</span></div>'
      + '<div class="sub" style="padding:8px 2px">' + (isC ? "还没有 Claude 的供应商。" : "还没有供应商。") + '<br>' + (loggedIn() ? "登录后一键导入,或点下方「＋ 新建」。" : "登录 2xapi 一键导入,或点下方「＋ 新建」。") + '</div>'
      + '<button class="btn ghost" data-a="new" style="width:100%;margin-top:8px">＋ 新建供应商</button>';
    return;
  }
  el.innerHTML =
    '<div class="rail-head"><span class="eyebrow">' + who + ' 供应商</span><span class="tag">共 ' + mine.length + '</span></div>'
    + '<input class="rail-search" data-a="search" placeholder="筛选名称或地址…" value="' + esc(state.search) + '">'
    + '<div id="railRows"></div>'
    + '<button class="btn ghost" data-a="new" style="width:100%;margin-top:8px">＋ 新建供应商</button>'
    + '<div class="sub" style="margin:8px 2px 0">这套列表只属于 ' + who + ',两边互相看不见。</div>';
  renderRailRows();
}
function railRowsHtml() {
  var q = state.search;
  var mine = providersFor(state.agent);
  var list = mine.filter(function (p) {
    return !q || (p.name || "").toLowerCase().includes(q) || (p.baseUrl || "").toLowerCase().includes(q);
  });
  if (!list.length) return '<div class="sub" style="padding:8px 2px">没有匹配的供应商。</div>';
  return list.map(function (p) {
    var i = mine.indexOf(p);
    return '<button class="line-row ' + (p.id === state.selId ? "sel" : "") + '" style="--lc:' + chipColor(p, i) + '" data-a="sel" data-id="' + esc(p.id) + '">'
      + (function () {
          var pre = (window.PRESETS || []).find(function (x) { return x.id === (p.icon || "") });
          return pre
            ? '<span class="line-chip logo"><img src="' + esc(pre.logo) + '" alt=""></span>'
            : '<span class="line-chip">' + esc(p.icon || String(i + 1)) + '</span>';
        })()
      + '<span class="nm">' + esc(p.name) + '</span>'
      + (hostedBy(p.id) ? '<span class="tag on">托管中</span>' : '')
      + '<span class="mini-op" data-a="edit" data-id="' + esc(p.id) + '" title="编辑">✎</span></button>';
  }).join("");
}
function renderRailRows() {
  var r = document.getElementById("railRows"); if (r) r.innerHTML = railRowsHtml();
}
function renderContent() {
  var c = document.getElementById("content"); if (!c) return;
  if (state.view === "history") c.innerHTML = historyHtml();
  else if (state.view === "eco") c.innerHTML = ecoCenterHtml();
  else if (state.view === "plug") c.innerHTML = plugCenterHtml();
  else c.innerHTML = (state.agent === "claude") ? claudeDashHtml()
    : (state.agent === "hermes") ? hermesDashHtml()
    : GW_AGENTS[state.agent] ? genericDashHtml(state.agent)
    : dashHtml();
}
function renderTopAuth() {
  var el = document.getElementById("topAuth"); if (!el) return;
  if (!loggedIn()) {
    el.innerHTML = '<button class="btn primary" data-a="login">登录 2xapi</button>'
      + '<span class="chip" style="margin-left:6px">登录即自动引导导入你的 API Key</span>';
    return;
  }
  var email = sessionEmail();
  var initial = (email || "?").slice(0, 1).toUpperCase();
  var bal = state.balance;
  var balHtml;
  if (!state.balShow) balHtml = '余额 <b>…</b>';
  else if (bal == null) balHtml = '余额 <b>…</b>';
  else if (bal < 1) balHtml = '余额 <b style="color:var(--c-err)">$' + bal.toFixed(2) + '</b>';
  else balHtml = '余额 <b style="color:var(--c-official)">$' + bal.toFixed(2) + '</b>';
  var topBal = "";
  if (state.balShow) {
    var v = (bal == null) ? "…" : "$" + bal.toFixed(2);
    var low = (bal != null && bal < 1) ? " low" : "";
    topBal = '<button class="bal-top' + low + '" data-a="user-menu" title="2xapi 账号余额 · 点击打开账号菜单(设置→账号 可隐藏)">' + v + '</button>';
  }
  el.innerHTML = topBal + '<div class="userbox">'
    + '<button class="avatar" data-a="user-menu" title="账号菜单">' + esc(initial) + '</button>'
    + (state.menuOpen
      ? '<div class="user-menu">'
        + '<div class="um-head"><div class="um-ava">' + esc(initial) + '</div><div><div class="um-mail">' + esc(email) + '</div><div class="um-bal">' + balHtml + '</div></div></div>'
        + '<button class="um-item primary-item" data-a="import-keys">⇭ 一键导入 Key<span class="um-sub">自动拉取账号下的 Key 建成供应商</span></button>'
        + '<button class="um-item" data-a="settings-open">⚙ 设置<span class="um-sub">IP 管理 · 通用 · 高级</span></button>'
        + '<button class="um-item" data-a="site">↗ 2xapi 官网<span class="um-sub">充值 / 管理 Key</span></button>'
        + '<div class="um-sep"></div>'
        + '<button class="um-item um-logout" data-a="logout">登出</button>'
      + '</div>'
      : '')
    + '</div>';
}
function render() {
  var mine = providersFor(state.agent);
  if (mine.length && !lineOf(state.selId)) {
    var h = hosting();
    state.selId = (h && h.providerId && lineOf(h.providerId)) ? h.providerId : mine[0].id;
  }
  renderNav();
  if (state.view !== "history") renderRail();
  renderContent();
  renderTopAuth();
  assertRouteShape();
}
/* 通路图形状自检:节点数 = 连线数 + 1 */
function assertRouteShape() {
  var st = document.querySelectorAll("#content .route > .st").length;
  var lk = document.querySelectorAll("#content .route > .lk").length;
  if (st && st !== lk + 1) console.warn("通路图形状异常: 节点 " + st + " ≠ 连线 " + lk + " + 1");
}

/* ── 主卡 dash(Codex)── */
function dashHtml() {
  var mine = providersFor("codex");
  if (!mine.length) {
    return '<section class="card" style="min-height:100%;display:flex;flex-direction:column;align-items:center;justify-content:center;text-align:center;gap:10px">'
      + '<div style="font-size:30px">🚀</div>'
      + '<h2 style="font-size:15px">开始使用 Codex</h2>'
      + '<div class="sub" style="max-width:380px">还没有供应商。' + (loggedIn() ? '点「导入 Key」自动生成供应商,' : '登录 2xapi 后一键导入 Key,') + '或手动添加一个中转站。</div>'
      + '<div class="btn-row" style="justify-content:center">'
      + (loggedIn() ? '<button class="btn primary" data-a="import-keys">⇭ 导入 Key</button>' : '<button class="btn primary" data-a="login">登录 2xapi</button>')
      + '<button class="btn" data-a="new">＋ 新建供应商</button></div></section>' + ecoStripHtml("codex");
  }
  var h = hosting();
  var hp = h ? lineOf(h.providerId) : null;
  var acc = state.accel || {};
  var accelMode = acc.mode || "off";
  var accelOn = accelMode !== "off";
  var hasOff = !!(state.dstate && state.dstate.hasOfficial);
  /* 直连门控:hasOfficial === false(纯 API 模式)才放开;官方登录在/状态未知 → 禁用 */
  var directOk = !!(state.dstate && state.dstate.hasOfficial === false);
  var way = codexWayNow();
  var direct = way === "direct";

  var st = function (c, b, s) { return '<div class="st" style="--lc:' + c + '"><span class="dot"></span><span class="lb"><b>' + b + '</b><span>' + s + '</span></span></div>'; };
  var lk = function (c) { return '<div class="lk live" style="--lc:' + c + '"></div>'; };
  var r, note;
  if (!hp) {
    r = st("var(--c-official)", "桌面版 Codex", "官方登录") + lk("var(--c-official)") + st("var(--c-official)", "官方 OpenAI", "chatgpt 登录");
    note = '当前:官方直连 · 选一个供应商并「开启托管」即可走中转';
  } else if (direct) {
    /* 通路:直连 —— Key 写入本地配置,不经网关、无加速(两站,同构 Claude 世界 direct 分支) */
    r = st("var(--c-gw)", "桌面版 Codex", "官方登录保留") + lk("var(--c-direct)") + st(chipColor(hp, mine.indexOf(hp)), esc(hp.name), "中转站");
    note = '通路:直连(Key 写入本地配置,不经网关、无加速)';
  } else if (!accelOn) {
    r = st("var(--c-gw)", "桌面版 Codex", "官方登录保留") + lk("var(--c-gw)") + st("var(--c-gw)", "网关", "127.0.0.1:8787") + lk("var(--c-gw)") + st(chipColor(hp, mine.indexOf(hp)), esc(hp.name), "中转站");
    note = '通路:网关(加速已关,直发上游) · 配置零 Key,Key 由网关注入';
  } else {
    r = st("var(--c-gw)", "桌面版 Codex", "官方登录保留") + lk("var(--c-gw)") + st("var(--c-gw)", "网关", "127.0.0.1:8787")
      + lk("var(--c-accel)") + st("var(--c-accel)", "加速节点", "自动择优线路") + lk("var(--c-accel)") + st(chipColor(hp, mine.indexOf(hp)), esc(hp.name), "中转站");
    note = '通路:网关 + 加速(已启用线路自动择优,失败自动切换) · 配置零 Key,Key 由网关注入';
  }
  /* scope 提示条(琥珀,后端给定文案;直连无加速 → 不出条) */
  var scopeHtml = "";
  if (hp && !direct && accelOn && acc.scopeNote) {
    scopeHtml = '<div style="margin:8px 0 0;padding:8px 10px;background:rgba(229,161,59,.08);border:1px solid rgba(229,161,59,.35);border-radius:8px;font-size:11.5px;color:#EAC98F">⚠ ' + esc(acc.scopeNote) + '</div>';
  }
  var usage = (acc.usage && acc.usage.ok) ? acc.usage : null;
  if (hp && !direct && accelOn && usage && usage.degradedToDirect) {
    scopeHtml += '<div style="margin:6px 0 0;padding:8px 10px;background:rgba(229,161,59,.08);border:1px solid rgba(229,161,59,.35);border-radius:8px;font-size:11.5px;color:#EAC98F">⚠ 官方加速配额已用满,已自动切换直连;可在 ⚙ 设置 → IP 管理 刷新凭证重试。</div>';
  }
  var selVal = hp ? hp.id : state.selId;
  var opts = mine.map(function (x) {
    return '<option value="' + esc(x.id) + '"' + (x.id === selVal ? " selected" : "") + '>' + esc(x.name) + (x.model ? "(" + esc(x.model) + ")" : "") + '</option>';
  }).join("");
  var p = lineOf(state.selId) || hp;
  var wayTagColor = direct ? "var(--c-direct)" : "var(--c-gw)";

  /* 详情卡在前、主卡在后(两世界一致) */
  var html = "";
  if (p) html += providerDetailCard(p);
  html += '<section class="card"><h2>桌面版 Codex(ChatGPT.app)· 主通道</h2>'
    + '<div class="detect">'
    + (hasOff
      ? '<span class="tag on">检测:官方登录 ✓ → 混入模式</span>'
      : '<span class="tag">检测:未检出官方登录 → 纯 API 模式</span>')
    + (hp ? '<span class="tag" style="border-color:var(--c-gw);color:var(--c-gw)">已托管 · ' + esc(hp.name) + '</span>' : '<span class="tag">未托管</span>')
    + '</div>'
    + '<div class="route">' + r + '</div>'
    + '<div class="route-mode"><span class="k">●</span> ' + note + '</div>'
    + scopeHtml
    + '<div class="grid2">'
    + '<div class="f"><label>通路方式</label><div class="seg">'
    + (directOk
      ? '<button data-a="way" data-w="direct" aria-pressed="' + direct + '" style="--lc:var(--c-direct)">直连<small>Key 写入本地配置</small></button>'
      : '<button disabled style="--lc:var(--c-direct)">直连<small>' + (hasOff ? "官方登录下直连暂不支持(待实测)" : "即将支持") + '</small></button>')
    + '<button data-a="way" data-w="gateway" aria-pressed="' + (!direct) + '" style="--lc:var(--c-gw)">网关 + 加速<small>零 Key(默认)</small></button></div></div>'
    + '<div class="f"><label>加速</label><div style="display:flex;gap:6px;align-items:center">'
    + '<div class="seg" style="flex:1">'
    + '<button data-a="accel" data-m="off" aria-pressed="' + (!accelOn) + '" style="--lc:var(--muted)"' + (direct ? " disabled" : "") + '>关</button>'
    + '<button data-a="accel" data-m="on" aria-pressed="' + (accelOn) + '" style="--lc:var(--c-accel)"' + (direct ? " disabled" : "") + '>开<small>自动择优</small></button></div>'
    + '</div><div class="sub" style="margin-top:2px">线路在 左下 ⚙ 设置 → IP 管理 里维护</div></div>'
    + '<div class="f"><label>供应商(走哪家中转)</label><select id="provSel" data-a="prov">' + opts + '</select></div>'
    + '<div class="f"><label>状态</label><div style="padding:6px 0"><span class="tag" style="border-color:' + wayTagColor + ';color:' + wayTagColor + '">' + (hp ? (direct ? "直连 · Key 写入本地配置" : "网关 · 配置零 Key") : "未托管") + '</span></div></div>'
    + '</div>'
    + '<div class="btn-row">'
    + (hp ? '<button class="btn" data-a="unhost"' + (state.busy === "unhost" ? " disabled" : "") + '>还原官方</button>'
      : '<button class="btn primary" data-a="host-on"' + (state.busy === "host" ? " disabled" : "") + '>开启托管</button>')
    + '<button class="btn ghost" data-a="test"' + (state.test && state.test.busy ? " disabled" : "") + '>⚡ 测试连接</button>'
    + '</div><div id="rtest"></div></section>';
  if (state.test) html = html.replace('<div id="rtest"></div>', testStepsHtml());
  return html + ecoStripHtml("codex");
}

/* 供应商详情卡(Codex / Claude 共用;按 agent 过滤与分支按钮) */
function providerDetailCard(p) {
  var hb = hostedBy(p.id);
  var tagLabel = state.agent === "claude" ? "注入中" : "托管中";
  var btns;
  if (state.agent === "claude") {
    /* Claude 世界:启用(未注入)/停用(已注入) + 编辑 + 诊断 + 删除(注入中禁用) */
    btns = (hb
      ? '<button class="btn" data-a="claude-stop" data-id="' + esc(p.id) + '"' + (state.busy === "claude-stop" ? " disabled" : "") + '>停用</button>'
      : '<button class="btn primary" data-a="claude-start" data-id="' + esc(p.id) + '"' + (state.busy === "claude-start" ? " disabled" : "") + '>启用</button>')
      + '<button class="btn" data-a="edit" data-id="' + esc(p.id) + '">编辑</button>'
      + '<button class="btn" data-a="diag">' + (state.diag && state.diag.forId === p.id ? "收起诊断" : "诊断") + '</button>'
      + (hb ? '<button class="btn ghost" disabled>删除(注入中)</button>' : '<button class="btn ghost danger" data-a="del" data-id="' + esc(p.id) + '">删除</button>');
  } else {
    /* Codex 世界:启用(未托管)/停用(已托管)+ 编辑 + 诊断 + 删除(托管中禁用)——与 Claude 分支同构 */
    btns = (hb
      ? '<button class="btn" data-a="unhost"' + (state.busy === "unhost" ? " disabled" : "") + '>停用</button>'
      : '<button class="btn primary" data-a="host-on" data-id="' + esc(p.id) + '"' + (state.busy === "host" ? " disabled" : "") + '>启用</button>')
      + '<button class="btn" data-a="edit" data-id="' + esc(p.id) + '">编辑</button>'
      + '<button class="btn" data-a="diag">' + (state.diag && state.diag.forId === p.id ? "收起诊断" : "诊断") + '</button>'
      + (hb ? '<button class="btn ghost" disabled>删除(' + tagLabel + ')</button>' : '<button class="btn ghost danger" data-a="del" data-id="' + esc(p.id) + '">删除</button>');
  }
  var html = '<section class="card"><div class="eyebrow" style="margin:0 0 2px">供应商详情 · ' + esc(p.name) + (hb ? ' <span class="tag on">' + tagLabel + '</span>' : '') + '</div>'
    + '<div class="kv">'
    + '<div><div class="k">上游地址</div><div class="v mono">' + esc(p.baseUrl) + '</div></div>'
    + '<div><div class="k">api key</div><div class="v mono">' + esc(p.apiKeyMasked || "—") + '</div></div>'
    + '<div><div class="k">协议</div><div class="v mono">' + esc(wireLabel(p)) + '</div></div>'
    + '<div><div class="k">默认模型</div><div class="v mono">' + esc(p.model || "—") + '</div></div>'
    + '</div>'
    + '<div class="btn-row">' + btns + '</div></section>';
  html += capsSectionHtml(p);
  if (state.diag && state.diag.forId === p.id) html += diagCard(state.diag.data);
  return html;
}

/* ── 主卡 dash(Claude:注入式托管)── */
function claudeEmptyHtml() {
  return '<section class="card" style="min-height:100%;display:flex;flex-direction:column;align-items:center;justify-content:center;text-align:center;gap:10px">'
    + '<svg viewBox="0 0 24 24" width="40" height="40" fill="var(--c-claude)" aria-hidden="true"><path d="m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z"/></svg>'
    + '<h2 style="font-size:15px">Claude Code · 接入预览</h2>'
    + '<div class="sub" style="max-width:400px">还没有 Claude 供应商。点下方「新建」添加一个 Anthropic 兼容中转站;这套列表只属于 Claude,与 Codex 完全独立。</div>'
    + '<div class="btn-row" style="justify-content:center"><button class="btn primary" data-a="new">＋ 新建供应商</button></div></section>';
}
function maskKey(k) {
  var s = String(k == null ? "" : k);
  if (!s) return "";
  if (s.length <= 8) return s.slice(0, 2) + "…" + s.slice(-2);
  return s.slice(0, 6) + "…" + s.slice(-4);
}
function claudeEnvHtml() {
  var c = state.claude;
  if (!c || !c.started) return "";
  var p = c.providerId ? lineOf(c.providerId) : null;
  var env = c.env || {};
  var base = env.ANTHROPIC_BASE_URL || env.baseUrl || "http://127.0.0.1:8787/anthropic";
  var rawTok = env.ANTHROPIC_AUTH_TOKEN || "";
  var tok = rawTok ? maskKey(rawTok) : (env.authTokenMasked || (p && p.apiKeyMasked) || "");
  var model = env.ANTHROPIC_MODEL || env.model || (p && p.model) || c.model || "";
  var rows = '<div><div class="k">ANTHROPIC_BASE_URL</div><div class="v mono">' + esc(base) + '</div></div>';
  if (tok) rows += '<div><div class="k">ANTHROPIC_AUTH_TOKEN</div><div class="v mono">' + esc(tok) + '</div></div>';
  if (model) rows += '<div><div class="k">ANTHROPIC_MODEL</div><div class="v mono">' + esc(model) + '</div></div>';
  /* 可复制启动命令:界面掩码展示,「复制」取完整命令(含 Key)进剪贴板 */
  var cmdHtml = "";
  if (c.command) {
    var disp = rawTok ? c.command.split(rawTok).join(maskKey(rawTok)) : c.command;
    cmdHtml = '<div style="margin-top:8px;display:flex;gap:6px;align-items:center">'
      + '<code style="flex:1;min-width:0;overflow-x:auto;white-space:nowrap;font:11px var(--mono);color:var(--text);background:var(--ink);border:1px solid var(--hair);border-radius:7px;padding:6px 9px">' + esc(disp) + '</code>'
      + '<button class="btn sm ghost" data-a="claude-copy" title="复制完整启动命令(含 Key)到剪贴板">复制</button></div>';
  }
  return '<div class="sub" style="margin:10px 0 0;padding:8px 10px;background:var(--raised);border:1px solid var(--hair);border-radius:8px">'
    + '<div class="eyebrow" style="margin:0 0 6px">已注入环境变量(Key 已掩码,不显明文)</div>'
    + '<div class="kv" style="margin-top:0">' + rows + '</div>' + cmdHtml + '</div>';
}
function claudeDashHtml() {
  var mine = providersFor("claude");
  if (!mine.length) return claudeEmptyHtml();
  var started = claudeStarted();
  var hp = started ? (lineOf(state.claude.providerId) || null) : null;
  started = !!hp; /* 注入的供应商已删除 → 视为未启动,回到「启动」态 */
  var acc = state.accel || {};
  var accelMode = acc.mode || "off";
  var accelOn = accelMode !== "off";
  var ac = "var(--c-claude)";

  var st = function (c, b, s) { return '<div class="st" style="--lc:' + c + '"><span class="dot"></span><span class="lb"><b>' + b + '</b><span>' + s + '</span></span></div>'; };
  var lk = function (c) { return '<div class="lk live" style="--lc:' + c + '"></div>'; };
  var way = claudeWay();
  var direct = way === "direct";
  var r, note;
  if (!hp) {
    r = st(ac, "Claude Code", "终端 CLI") + lk(ac) + st(ac, "官方 Anthropic", "claude.ai 登录");
    note = '当前:官方直连 · 选一个供应商并点「启用」即注入中转';
  } else if (direct) {
    /* 通路:直连 —— Key 直注入,不经网关、无加速(两站) */
    r = st(ac, "Claude Code", "注入式") + lk("var(--c-direct)") + st(chipColor(hp, mine.indexOf(hp)), esc(hp.name), "中转站");
    note = '通路:直连(Key 直注入,不经网关、无加速)';
  } else if (!accelOn) {
    /* 通路:网关,加速关(三站) */
    r = st(ac, "Claude Code", "注入式") + lk(ac) + st("var(--c-gw)", "网关", "127.0.0.1:8787") + lk(ac) + st(chipColor(hp, mine.indexOf(hp)), esc(hp.name), "中转站");
    note = '通路:网关(注入式,不写 ~/.claude 配置)';
  } else {
    /* 通路:网关 + 加速(四站) */
    r = st(ac, "Claude Code", "注入式") + lk(ac) + st("var(--c-gw)", "网关", "127.0.0.1:8787")
      + lk("var(--c-accel)") + st("var(--c-accel)", "加速节点", "自动择优线路") + lk("var(--c-accel)") + st(chipColor(hp, mine.indexOf(hp)), esc(hp.name), "中转站");
    note = '通路:网关(注入式,不写 ~/.claude 配置)';
  }
  /* scope 提示条(与 Codex 同逻辑,agent=claude 时 scopeNote 语义同;直连无加速 → 不出条) */
  var scopeHtml = "";
  if (hp && !direct && accelOn && acc.scopeNote) {
    scopeHtml = '<div style="margin:8px 0 0;padding:8px 10px;background:rgba(229,161,59,.08);border:1px solid rgba(229,161,59,.35);border-radius:8px;font-size:11.5px;color:#EAC98F">⚠ ' + esc(acc.scopeNote) + '</div>';
  }
  var usage = (acc.usage && acc.usage.ok) ? acc.usage : null;
  if (hp && !direct && accelOn && usage && usage.degradedToDirect) {
    scopeHtml += '<div style="margin:6px 0 0;padding:8px 10px;background:rgba(229,161,59,.08);border:1px solid rgba(229,161,59,.35);border-radius:8px;font-size:11.5px;color:#EAC98F">⚠ 官方加速配额已用满,已自动切换直连;可在 ⚙ 设置 → IP 管理 刷新凭证重试。</div>';
  }
  var selVal = hp ? hp.id : state.selId;
  var opts = mine.map(function (x) {
    return '<option value="' + esc(x.id) + '"' + (x.id === selVal ? " selected" : "") + '>' + esc(x.name) + (x.model ? "(" + esc(x.model) + ")" : "") + '</option>';
  }).join("");
  var p = lineOf(state.selId) || hp;

  /* 详情卡在前、主卡在后(两世界一致) */
  var html = "";
  if (p) html += providerDetailCard(p);
  html += '<section class="card"><h2>Claude Code · 主通道(注入式)</h2>'
    + '<div class="detect">'
    + '<span class="tag on">Claude Code · 注入式</span>'
    + (started ? '<span class="tag" style="border-color:var(--c-claude);color:var(--c-claude)">已注入 · ' + esc(hp.name) + '</span>' : '<span class="tag">未注入</span>')
    + '</div>'
    + '<div class="route">' + r + '</div>'
    + '<div class="route-mode"><span class="k" style="color:var(--c-claude)">●</span> ' + note + '</div>'
    + scopeHtml
    + claudeEnvHtml()
    + '<div class="grid2">'
    + '<div class="f"><label>通路方式</label><div class="seg">'
    + '<button data-a="way" data-w="direct" aria-pressed="' + (direct) + '" style="--lc:var(--c-direct)">直连<small>Key 直注入</small></button>'
    + '<button data-a="way" data-w="gateway" aria-pressed="' + (!direct) + '" style="--lc:var(--c-gw)">网关<small>经网关·可加速</small></button></div></div>'
    + '<div class="f"><label>加速</label><div style="display:flex;gap:6px;align-items:center">'
    + '<div class="seg" style="flex:1">'
    + '<button data-a="accel" data-m="off" aria-pressed="' + (!accelOn) + '" style="--lc:var(--muted)"' + (direct ? " disabled" : "") + '>关</button>'
    + '<button data-a="accel" data-m="on" aria-pressed="' + (accelOn) + '" style="--lc:var(--c-accel)"' + (direct ? " disabled" : "") + '>开<small>自动择优</small></button></div>'
    + '</div><div class="sub" style="margin-top:2px">线路在 左下 ⚙ 设置 → IP 管理 里维护</div></div>'
    + '<div class="f"><label>供应商(走哪家中转)</label><select id="provSel" data-a="prov">' + opts + '</select></div>'
    + '<div class="f"><label>状态</label><div style="padding:6px 0"><span class="tag" style="border-color:' + (direct ? "var(--c-direct)" : "var(--c-gw)") + ';color:' + (direct ? "var(--c-direct)" : "var(--c-gw)") + '">' + (started ? (direct ? "直连 · 已注入" : "网关 · 已注入") : "未启动") + '</span></div></div>'
    + '</div>'
    + '<div class="btn-row">'
    + (started
      ? '<button class="btn" data-a="claude-stop"' + (state.busy === "claude-stop" ? " disabled" : "") + '>还原官方</button>'
      : '<button class="btn primary" data-a="claude-start"' + (state.busy === "claude-start" ? " disabled" : "") + '>启动 Claude Code</button>')
    + '<button class="btn ghost" data-a="test"' + (state.test && state.test.busy ? " disabled" : "") + '>⚡ 测试连接</button>'
    + '</div><div id="rtest"></div></section>';
  if (state.test) html = html.replace('<div id="rtest"></div>', testStepsHtml());
  return html + ecoStripHtml("claude");
}

/* 测试连接:三段 steps(密钥 / 协议 / 建议) */
function testStepsHtml() {
  var t = state.test;
  var step = function (icon, text, meta, bad) {
    return '<div class="step' + (bad ? " bad" : "") + '">' + icon + " " + text + (meta ? '<span class="meta">' + esc(meta) + "</span>" : "") + "</div>";
  };
  if (t.busy) {
    return '<div id="rtest"><div class="steps" style="margin-top:12px">' + step("⟳", "测试连接进行中…", "密钥/协议/建议") + "</div></div>";
  }
  if (!t.ok) {
    return '<div id="rtest"><div class="steps" style="margin-top:12px">' + step("✗", t.msg || "测试连接失败", "", true) + "</div></div>";
  }
  var d = t.data;
  var steps = [];
  steps.push(step(d.keyOk ? "✓" : "✗", d.keyOk ? "密钥有效" : "密钥无效", (d.keyOk ? ((d.models || []).length + " 个模型") : "") + " · " + (d.latencyMs != null ? d.latencyMs + "ms" : ""), !d.keyOk));
  var proto = d.responsesCompat ? "Responses 兼容" : (d.chatOk ? "仅 Chat(网关自动转换)" : "协议未测出");
  steps.push(step((d.responsesCompat || d.chatOk) ? "✓" : "✗", "协议判定:" + proto, d.responsesCompat ? "免转换" : (d.chatOk ? "需经网关转换" : ""), !(d.responsesCompat || d.chatOk)));
  if (d.suggest === "gateway") steps.push(step("⚡", "建议方式:网关(推荐,零落盘)", "可一键开启托管"));
  else if (d.error) steps.push(step("✗", "无可用接入方式", d.error, true));
  else steps.push(step("⚡", "建议方式:网关", ""));
  return '<div id="rtest"><div class="steps" style="margin-top:12px">' + steps.join("") + "</div></div>";
}
async function doTestConnection() {
  var pid = (hosting() && hosting().providerId) || state.selId;
  if (!pid) { showToast("请先选择或新建一个供应商", "error"); return; }
  state.test = { busy: true }; render();
  try {
    var d = await api.preflight({ providerId: pid });
    state.test = { ok: true, data: d };
  } catch (e) {
    state.test = { ok: false, msg: e.message };
  }
  render();
}

function diagCard(d) {
  var ok = function (b) { return b ? "✓" : "✗"; };
  var cls = function (b) { return b ? "" : " bad"; };
  if (!d) {
    return '<section class="card"><div class="eyebrow" style="margin:0 0 10px">诊断 / doctor</div><div class="steps" style="margin-top:0">'
      + '<div class="step">⟳ 诊断进行中…<span class="meta">连接测试 + 真实请求</span></div></div></section>';
  }
  var errs = (d.errors || []).map(function (e) { return esc(e.message || e.msg || String(e)); }).join(";");
  return '<section class="card"><div class="eyebrow" style="margin:0 0 10px">诊断 / doctor</div><div class="steps" style="margin-top:0">'
    + '<div class="step' + cls(d.configValid) + '">' + ok(d.configValid) + ' 配置校验<span class="meta">' + (d.configValid ? "pass" : "fail") + '</span></div>'
    + '<div class="step' + cls(d.reachable) + '">' + ok(d.reachable) + ' 连接测试<span class="meta">' + (d.reachable ? ((d.latencyMs != null ? d.latencyMs + "ms · " : "") + (d.models || []).length + " models") : "不通") + '</span></div>'
    + '<div class="step' + cls(d.testOk) + '">' + ok(d.testOk) + ' 真实请求<span class="meta">' + (d.testOk ? "pass" : "fail") + '</span></div>'
    + '</div>' + (errs ? '<div style="margin:8px 0 0;padding:8px 10px;background:rgba(226,88,78,.08);border:1px solid rgba(226,88,78,.4);border-radius:8px;font-size:11.5px;color:#FFBAB4">' + errs + '</div>' : "")
    + '</section>';
}

/* ── 主卡 dash(Hermes:叠加式托管,条目写入 ~/.hermes/config.yaml;指针受控切换)── */
/* ── 通用平台世界(「全部做好」批次):gemini/grokbuild/opencode/openclaw/claude-desktop/workbuddy
 * 共享一个数据驱动的世界视图;后端契约=泛化路由 state/host/unhost(叠加或受控段托管,按 adapter 语义)。── */
var GW_META = {
  "gemini":       { label: "Gemini CLI", emoji: "✦", gw: "127.0.0.1:8787(生成协议转换)", overlay: "托管写入 ~/.gemini(.env 占位 Key + 认证类型),还原恢复快照;当前版本仅文本与工具调用(图片/文件会明确报错);需已安装 gemini CLI(npm i -g @google/gemini-cli)", start: true },
  "grokbuild":    { label: "Grok Build", emoji: "𝕏", gw: "127.0.0.1:8787/grokbuild", overlay: "受控段写入 ~/.grok/config.toml([models]/[model.*]),已有其他段零触碰;还原按快照受控恢复;需已安装 Grok CLI(npm i -g @xai-official/grok)", start: true },
  "opencode":     { label: "OpenCode", emoji: "◐", gw: "127.0.0.1:8787/opencode", overlay: "叠加条目写入 opencode.json(provider.2xapi-gateway),已有供应商与插件零触碰;默认模型仅空缺时才接" },
  "openclaw":     { label: "OpenClaw", emoji: "🐾", gw: "127.0.0.1:8787/openclaw", overlay: "叠加条目写入 openclaw.json(models.providers),OpenClaw 自管理的派生注册表不碰;默认模型仅空缺时才接" },
  "claude-desktop": { label: "Claude 桌面版", emoji: "◇", gw: "127.0.0.1:8787/claude-desktop", overlay: "官方原生 3p 网关 profile(配置库写入);改配置后需重启 Claude Desktop 生效", restart: true },
  "workbuddy":    { label: "WorkBuddy / CodeBuddy", emoji: "◆", gw: "127.0.0.1:8787/workbuddy", overlay: "叠加条目写入 models.json(双载体同步),已有条目零触碰" },
  "cursor":       { label: "Cursor", emoji: "⌘", gw: "127.0.0.1:8787/cursor", overlay: "托管写入 Cursor 的 state.vscdb(aiSettings 两字段 + Key 明文自动迁移钥匙串),登录态与其他配置零触碰;托管需先退出 Cursor;还原按快照精确恢复", restart: true }
};
var GW_AGENTS = {}; Object.keys(GW_META).forEach(function (k) { GW_AGENTS[k] = 1; });
function gwState(agent) { return (state.gw && state.gw[agent]) || null; }
function gwHosted(agent) { var s = gwState(agent); return !!(s && s.hosting); }

function refreshGwState(agent) {
  state.gw = state.gw || {};
  return api.agentState(agent).then(function (v) { state.gw[agent] = v; }).catch(function () { state.gw[agent] = null; });
}

function genericDashHtml(agent) {
  var meta = GW_META[agent] || { label: agent, emoji: "◆", gw: "127.0.0.1:8787/" + agent, overlay: "" };
  var mine = providersFor(agent);
  if (!mine.length) {
    return '<section class="card" style="min-height:100%;display:flex;flex-direction:column;align-items:center;justify-content:center;text-align:center;gap:10px">'
      + '<div style="font-size:30px">' + meta.emoji + '</div>'
      + '<h2 style="font-size:15px">开始使用 ' + esc(meta.label) + '</h2>'
      + '<div class="sub" style="max-width:380px">还没有 ' + esc(meta.label) + ' 供应商。' + (loggedIn() ? '点「导入 Key」自动生成供应商,' : '登录 2xapi 后一键导入 Key,') + '或手动添加一个中转站。</div>'
      + '<div class="btn-row" style="justify-content:center">'
      + (loggedIn() ? '<button class="btn primary" data-a="import-keys">⇭ 导入 Key</button>' : '<button class="btn primary" data-a="login">登录 2xapi</button>')
      + '<button class="btn" data-a="new">＋ 新建供应商</button></div></section>' + ecoStripHtml(agent);
  }
  var hosted = gwHosted(agent);
  var acc = state.accel || {};
  var accelOn = (acc.mode || "off") !== "off";
  var hp = hosted ? (lineOf(state.selId) || mine[0]) : null;
  var st = function (c, b, s) { return '<div class="st" style="--lc:' + c + '"><span class="dot"></span><span class="lb"><b>' + b + '</b><span>' + s + '</span></span></div>'; };
  var lk = function (c) { return '<div class="lk live" style="--lc:' + c + '"></div>'; };
  var r, note;
  if (!hosted) {
    r = st("var(--c-official)", meta.label, "官方/自有配置") + lk("var(--c-official)") + st("var(--c-official)", "直连", "未托管");
    note = '当前:按自有配置直连 · 选一个供应商并「开启托管」即可走中转(' + esc(meta.overlay) + ')';
  } else if (!accelOn) {
    r = st("var(--c-gw)", meta.label, "托管条目 2xapi-gateway") + lk("var(--c-gw)") + st("var(--c-gw)", "网关", esc(meta.gw)) + lk("var(--c-gw)") + st(chipColor(hp, mine.indexOf(hp)), esc(hp.name), "中转站");
    note = '通路:网关(加速已关,直发上游) · 托管配置零真 Key,Key 由网关注入';
  } else {
    r = st("var(--c-gw)", meta.label, "托管条目 2xapi-gateway") + lk("var(--c-gw)") + st("var(--c-gw)", "网关", esc(meta.gw))
      + lk("var(--c-accel)") + st("var(--c-accel)", "加速节点", "自动择优线路") + lk("var(--c-accel)") + st(chipColor(hp, mine.indexOf(hp)), esc(hp.name), "中转站");
    note = '通路:网关 + 加速(线路自动择优,失败自动切换) · 托管配置零真 Key,Key 由网关注入';
  }
  var selVal = hp ? hp.id : (state.selId || (mine[0] && mine[0].id));
  var opts = mine.map(function (x) {
    return '<option value="' + esc(x.id) + '"' + (x.id === selVal ? " selected" : "") + '>' + esc(x.name) + (x.model ? "(" + esc(x.model) + ")" : "") + '</option>';
  }).join("");

  var html = "";
  var p = lineOf(selVal);
  if (p) html += providerDetailCard(p);
  html += '<section class="card"><h2>' + esc(meta.label) + ' · 主通道</h2>'
    + '<div class="detect">'
    + (hosted ? '<span class="tag" style="border-color:var(--c-gw);color:var(--c-gw)">已托管</span>' : '<span class="tag">未托管</span>')
    + (meta.restart && hosted ? '<span class="tag" style="border-color:#FF9E57;color:#FF9E57">重启客户端后生效</span>' : '')
    + '</div>'
    + '<div class="route">' + r + '</div>'
    + '<div class="route-mode"><span class="k">●</span> ' + note + '</div>'
    + '<div class="grid2">'
    + '<div class="f"><label>供应商(走哪家中转)</label><select id="provSel" data-a="prov">' + opts + '</select></div>'
    + '<div class="f"><label>状态</label><div style="padding:6px 0"><span class="tag" style="border-color:var(--c-gw);color:var(--c-gw)">' + (hosted ? "网关 · 托管中" : "未托管") + '</span></div></div>'
    + '</div>'
    + '<div class="btn-row">'
    + (hosted ? '<button class="btn" data-a="unhost"' + (state.busy === "unhost" ? " disabled" : "") + '>还原官方</button>'
      : '<button class="btn primary" data-a="host"' + (state.busy === "host" ? " disabled" : "") + '>开启托管</button>')
    + (meta.start ? '<button class="btn" data-a="gw-start"' + (state.busy === "gw-start" ? " disabled" : "") + '>⌘ 生成启动命令</button>' : '')
    + '</div></section>';
  return html + ecoStripHtml(agent);
}

function hermesDashHtml() {
  var mine = providersFor("hermes");
  if (!mine.length) {
    return '<section class="card" style="min-height:100%;display:flex;flex-direction:column;align-items:center;justify-content:center;text-align:center;gap:10px">'
      + '<div style="font-size:30px">🪽</div>'
      + '<h2 style="font-size:15px">开始使用 Hermes</h2>'
      + '<div class="sub" style="max-width:380px">还没有 Hermes 供应商。' + (loggedIn() ? '点「导入 Key」自动生成供应商,' : '登录 2xapi 后一键导入 Key,') + '或手动添加一个中转站(需 OpenAI Chat 兼容协议)。</div>'
      + '<div class="btn-row" style="justify-content:center">'
      + (loggedIn() ? '<button class="btn primary" data-a="import-keys">⇭ 导入 Key</button>' : '<button class="btn primary" data-a="login">登录 2xapi</button>')
      + '<button class="btn" data-a="new">＋ 新建供应商</button></div></section>' + ecoStripHtml("hermes");
  }
  var hosted = hermesHosted();
  var ptr = hermesPointerName();
  var acc = state.accel || {};
  var accelOn = (acc.mode || "off") !== "off";
  var hp = hosted ? (lineOf(state.selId) || mine[0]) : null;

  var st = function (c, b, s) { return '<div class="st" style="--lc:' + c + '"><span class="dot"></span><span class="lb"><b>' + b + '</b><span>' + s + '</span></span></div>'; };
  var lk = function (c) { return '<div class="lk live" style="--lc:' + c + '"></div>'; };
  var r, note;
  if (!hosted) {
    r = st("var(--c-official)", "Hermes CLI", "当前供应商:" + (esc(ptr) || "未设置")) + lk("var(--c-official)") + st("var(--c-official)", "官方/自有配置", "~/.hermes/config.yaml");
    note = '当前:Hermes 按自有配置直连 · 选一个供应商并「开启托管」即可走中转(写入叠加条目,不动已有配置)';
  } else if (!accelOn) {
    r = st("var(--c-gw)", "Hermes CLI", "条目 2xapi-gateway") + lk("var(--c-gw)") + st("var(--c-gw)", "网关", "127.0.0.1:8787/hermes") + lk("var(--c-gw)") + st(chipColor(hp, mine.indexOf(hp)), esc(hp.name), "中转站");
    note = '通路:网关(加速已关,直发上游) · 条目零真 Key,Key 由网关注入';
  } else {
    r = st("var(--c-gw)", "Hermes CLI", "条目 2xapi-gateway") + lk("var(--c-gw)") + st("var(--c-gw)", "网关", "127.0.0.1:8787/hermes")
      + lk("var(--c-accel)") + st("var(--c-accel)", "加速节点", "自动择优线路") + lk("var(--c-accel)") + st(chipColor(hp, mine.indexOf(hp)), esc(hp.name), "中转站");
    note = '通路:网关 + 加速(已启用线路自动择优,失败自动切换) · 条目零真 Key,Key 由网关注入';
  }
  var selVal = hp ? hp.id : (state.selId || (mine[0] && mine[0].id));
  var opts = mine.map(function (x) {
    return '<option value="' + esc(x.id) + '"' + (x.id === selVal ? " selected" : "") + '>' + esc(x.name) + (x.model ? "(" + esc(x.model) + ")" : "") + '</option>';
  }).join("");
  var p = lineOf(selVal);

  var html = "";
  if (p) html += providerDetailCard(p);
  html += '<section class="card"><h2>Hermes Agent · 主通道</h2>'
    + '<div class="detect">'
    + (hosted ? '<span class="tag" style="border-color:var(--c-gw);color:var(--c-gw)">已托管 · 叠加条目 2xapi-gateway</span>' : '<span class="tag">未托管</span>')
    + (ptr ? '<span class="tag">指针:' + esc(ptr) + '</span>' : '<span class="tag">指针:未设置</span>')
    + '</div>'
    + '<div class="route">' + r + '</div>'
    + '<div class="route-mode"><span class="k">●</span> ' + note + '</div>'
    + '<div style="margin:8px 0 0;padding:8px 10px;background:rgba(120,160,255,.06);border:1px solid rgba(120,160,255,.25);border-radius:8px;font-size:11.5px;color:#A8C0F0">ⓘ 叠加平台:条目写入 ~/.hermes/config.yaml 的 custom_providers,已有供应商与个性化配置零触碰;托管仅在指针为官方默认/未设置时自动切换默认模型,「还原官方」即移除条目并恢复指针。</div>'
    + '<div class="grid2">'
    + '<div class="f"><label>供应商(走哪家中转)</label><select id="provSel" data-a="prov">' + opts + '</select></div>'
    + '<div class="f"><label>状态</label><div style="padding:6px 0"><span class="tag" style="border-color:var(--c-gw);color:var(--c-gw)">' + (hosted ? "网关 · 叠加条目" : "未托管") + '</span></div></div>'
    + '</div>'
    + '<div class="btn-row">'
    + (hosted ? '<button class="btn" data-a="unhost"' + (state.busy === "unhost" ? " disabled" : "") + '>还原官方</button>'
      : '<button class="btn primary" data-a="host-on"' + (state.busy === "host" ? " disabled" : "") + '>开启托管</button>')
    + '</div></section>';
  return html + ecoStripHtml("hermes");
}

/* ── 历史会话视图(Codex 走 db 列表+修复;Claude 走 ~/.claude jsonl 只读列表)── */
/* ── 生态中心(开发组·生态中心 A 段):平台 tab + MCP 服务器管理 + 预设市场 ──
 * 后端契约:GET|POST /api/desktop/:agent/eco(信封 data={servers});手动条目只读(409 拒写)。
 * 总部修订(2026-08-17):三 tab 设计作废,生态管理=MCP 单页;平台清单来自 /api/desktop/eco-presets。 */
var ECO_STATE = { agent: "codex", data: null, presets: null, agentsList: null, loading: false, pendingInstall: null, pendingParams: {} };
/* eco 支持平台(与后端 SUPPORTED 同步;世界内嵌生态入口按此渲染) */
var ECO_SUPPORTED_IDS = { codex: 1, cursor: 1, "claude-desktop": 1, grokbuild: 1, opencode: 1, hermes: 1, trae: 1 };
function loadEco(force) {
  ECO_STATE.loading = true; render();
  var p = api.ecoPresets().then(function (v) {
    ECO_STATE.presets = (v && v.presets) || [];
    ECO_STATE.agentsList = (v && v.agents) || [];
  }).catch(function () { ECO_STATE.presets = []; ECO_STATE.agentsList = []; });
  var l = api.ecoList(ECO_STATE.agent).then(function (v) {
    ECO_STATE.data = v;
  }).catch(function (e) {
    ECO_STATE.data = { error: e.message };
  }).finally(function () { ECO_STATE.loading = false; });
  loadPlug();
  return Promise.all([p, l]).then(function () { if (state.view === "eco") render(); });
}
function ecoServers() { return (ECO_STATE.data && ECO_STATE.data.servers) || []; }
function ecoPresetInstalled(id) { return ecoServers().some(function (s) { return s.id === id; }); }
function ecoCenterHtml() {
  var agents = (ECO_STATE.agentsList && ECO_STATE.agentsList.length) ? ECO_STATE.agentsList : [{ id: "codex", name: "Codex" }, { id: "cursor", name: "Cursor" }];
  var html = '<section class="card">'
    + '<h2>🌱 生态中心 · MCP 服务器管理</h2>'
    + '<div class="sub" style="margin:4px 0 12px">为各平台管理工具生态:叠加写入 · 零触碰已有(手动添加的条目只读)· 备份先行 · 可随时卸载还原。</div>'
    + '<div class="eco-tabs">';
  agents.forEach(function (m) {
    html += '<button class="eco-tab ' + (ECO_STATE.agent === m.id ? "on" : "") + '" data-a="eco-agent" data-g="' + esc(m.id) + '">' + esc(m.name) + '</button>';
  });
  html += '</div>';
  if (ECO_STATE.loading) {
    html += '<div class="sub" style="padding:20px 0;text-align:center">加载中…</div></section>';
    return html;
  }
  if (ECO_STATE.data && ECO_STATE.data.error) {
    html += '<div style="padding:10px 12px;background:rgba(226,88,78,.08);border:1px solid rgba(226,88,78,.4);border-radius:8px;font-size:11.5px;color:#FFBAB4">'
      + esc(ECO_STATE.data.error) + '</div></section>';
    return html;
  }
  /* MCP 列表 */
  var rows = ecoServers().map(function (s) {
    var manual = s.source === "manual";
    var srcTag = manual
      ? '<span class="tag">手动添加</span>'
      : '<span class="tag on" style="border-color:var(--c-gw);color:var(--c-gw)">Console</span>';
    var stTag = s.enabled
      ? '<span class="tag" style="border-color:var(--c-official);color:var(--c-official)">● 已启用</span>'
      : '<span class="tag">○ 已停用</span>';
    var ops = manual
      ? '<span class="sub" style="font-size:10px">只读(手动条目)</span>'
      : (s.enabled
        ? '<button class="btn ghost" data-a="eco-op" data-op="disable" data-id="' + esc(s.id) + '">停用</button> '
        : '<button class="btn ghost" data-a="eco-op" data-op="enable" data-id="' + esc(s.id) + '">启用</button> ')
        + '<button class="btn ghost" data-a="eco-op" data-op="uninstall" data-id="' + esc(s.id) + '">卸载</button>';
    return '<tr><td><b>' + esc(s.id) + '</b></td><td class="eco-summary">' + esc(s.summary || "") + '</td>'
      + '<td>' + srcTag + '</td><td>' + stTag + '</td><td style="white-space:nowrap">' + ops + '</td></tr>';
  }).join("");
  html += '<table class="mtable"><thead><tr><th style="width:20%">MCP 服务器</th><th>命令 / 配置</th><th style="width:12%">来源</th><th style="width:12%">状态</th><th style="width:22%">操作</th></tr></thead>'
    + (rows ? '<tbody>' + rows + '</tbody>' : '<tbody><tr><td colspan="5" class="sub" style="padding:14px 4px">还没有 MCP 服务器;从下方市场一键安装,或在平台配置中手动添加(手动条目这里只读展示)。</td></tr></tbody>')
    + '</table>'
    + '<div class="sub" style="margin-top:8px">⚙ 每条 = 平台原生 MCP 配置里的一个条目;操作前自动备份,卸载可完整还原。已安装的 CLI 需重启后生效。</div>'
    + '</section>';
  /* 装时填参表单 */
  var pending = ECO_STATE.pendingInstall
    ? (ECO_STATE.presets || []).find(function (x) { return x.id === ECO_STATE.pendingInstall; })
    : null;
  var paramForm = "";
  if (pending && pending.params && pending.params.length) {
    var fields = pending.params.map(function (pm) {
      return '<div class="f" style="grid-column:1/-1"><label>' + esc(pm.label) + (pm.required ? " *" : "") + '</label>'
        + '<input class="mono" data-pk="' + esc(pm.key) + '" placeholder="' + esc(pm.placeholder || "") + '" value="' + esc(ECO_STATE.pendingParams[pm.key] || "") + '"></div>';
    }).join("");
    paramForm = '<section class="card" style="margin-top:12px"><h2>安装 ' + esc(pending.name) + ' · 需要参数</h2>'
      + '<div class="grid2" style="margin-top:6px">' + fields + '</div>'
      + '<div class="btn-row" style="margin-top:8px">'
      + '<button class="btn primary" data-a="eco-param-do" data-id="' + esc(pending.id) + '">安装</button>'
      + '<button class="btn ghost" data-a="eco-param-cancel">取消</button></div></section>';
  }
  /* 预设市场 */
  var cards = (ECO_STATE.presets || []).map(function (p) {
    var installed = ecoPresetInstalled(p.id);
    return '<div class="eco-card"><div class="ic">🔌</div><div style="min-width:0">'
      + '<b>' + esc(p.name) + '</b><span class="d">' + esc(p.desc || "") + ((p.needsEnv && p.needsEnv.length) ? ' · 需 ' + esc(p.needsEnv.join(", ")) : '') + '</span></div>'
      + (installed
        ? '<span class="sub" style="font-size:10px;flex:none">已安装</span>'
        : '<button class="eco-install" data-a="eco-install" data-id="' + esc(p.id) + '">点击安装</button>')
      + '</div>';
  }).join("");
  var cur = agents.find(function (m) { return m.id === ECO_STATE.agent; });
  html += paramForm;
  html += '<section class="card" style="margin-top:12px"><h2>MCP 预设市场</h2>'
    + '<div class="sub" style="margin:4px 0 10px">一键安装到当前平台(' + esc((cur && cur.name) || ECO_STATE.agent) + ');预设提炼自 cc-switch 与常用清单。</div>'
    + '<div class="eco-grid">' + (cards || '<span class="sub">暂无预设</span>') + '</div></section>';
  return html;
}
function doEcoOp(op, name) {
  ECO_STATE.busy = name || op;
  api.ecoOp(ECO_STATE.agent, { op: op, name: name }).then(function (v) {
    ECO_STATE.data = v;
    var label = { install: "安装", uninstall: "卸载", enable: "启用", disable: "停用" }[op] || op;
    showToast(label + (name ? " " + name : "") + "完成", "ok");
  }).catch(function (e) {
    showToast(e.message || "操作失败", "error");
  }).finally(function () {
    ECO_STATE.busy = null;
    if (state.view === "eco") render();
  });
}
function doEcoInstall(presetId, params) {
  var p = (ECO_STATE.presets || []).find(function (x) { return x.id === presetId; });
  ECO_STATE.busy = presetId;
  var body = { op: "install", presetId: presetId };
  if (params) body.params = params;
  api.ecoOp(ECO_STATE.agent, body).then(function (v) {
    ECO_STATE.data = v;
    var envNote = (p && p.needsEnv && p.needsEnv.length)
      ? ";该服务器需要 " + p.needsEnv.join(", ") + " 环境变量,已按空值安装,请稍后在平台配置中补填"
      : "";
    showToast("已安装到 " + ECO_STATE.agent + envNote, "ok");
  }).catch(function (e) {
    showToast(e.message || "安装失败", "error");
  }).finally(function () {
    ECO_STATE.busy = null;
    ECO_STATE.pendingInstall = null;
    ECO_STATE.pendingParams = {};
    if (state.view === "eco") render();
  });
}

/* ── 插件与能力市场(多模态引擎部 二期):官方能力条目 + http 插件 + 第三方源管理 ──
 * 后端契约(raw_json 形态):GET /api/plugins{plugins}+GET /api/plugin-market{sources,official};
 * 调用入口 POST /api/plugins/:id/invoke(M3 契约,错误 200 包错误带 human)。 */
var PLUG_STATE = { market: null, installed: null, loading: false, srcForm: false, busy: null, browse: null };
function loadPlug() {
  PLUG_STATE.loading = true;
  var m = api.plugMarket().then(function (v) { PLUG_STATE.market = v; })
    .catch(function (e) { PLUG_STATE.market = { error: e.message }; });
  var i = api.plugList().then(function (v) { PLUG_STATE.installed = (v && v.plugins) || []; })
    .catch(function () { PLUG_STATE.installed = []; });
  return Promise.all([m, i]).then(function () {
    PLUG_STATE.loading = false;
    if (state.view === "eco") render();
  });
}
function plugInstalled(id) { return (PLUG_STATE.installed || []).some(function (x) { return x.id === id; }); }
function doPlugInstall(sourceId, pluginId) {
  PLUG_STATE.busy = pluginId;
  api.plugInstall(sourceId, pluginId).then(function () {
    showToast("已安装 " + pluginId, "ok");
  }).catch(function (e) { showToast(e.message || "安装失败", "error"); })
    .finally(function () { PLUG_STATE.busy = null; loadPlug(); });
}
function doPlugOp(op, id) {
  PLUG_STATE.busy = id;
  var done = op === "remove" ? api.plugRemove(id) : api.plugToggle(id, op === "enable");
  done.then(function () {
    showToast(({ enable: "已启用", disable: "已停用", remove: "已卸载" })[op] || op, "ok");
  }).catch(function (e) { showToast(e.message || "操作失败", "error"); })
    .finally(function () { PLUG_STATE.busy = null; loadPlug(); });
}
function plugSectionHtml() {
  if (PLUG_STATE.loading && !PLUG_STATE.market) {
    return '<section class="card" style="margin-top:12px"><h2>🔌 插件与能力市场</h2>'
      + '<div class="sub" style="padding:14px 0;text-align:center">加载中…</div></section>';
  }
  var mk = PLUG_STATE.market || {};
  if (mk.error) {
    return '<section class="card" style="margin-top:12px"><h2>🔌 插件与能力市场</h2>'
      + '<div style="padding:10px 12px;background:rgba(226,88,78,.08);border:1px solid rgba(226,88,78,.4);border-radius:8px;font-size:11.5px;color:#FFBAB4">' + esc(mk.error) + '</div></section>';
  }
  var html = "";
  /* 已装列表(plugin=HTTP 型,tool=官方内置能力) */
  var rows = (PLUG_STATE.installed || []).map(function (e) {
    var meta = e.meta || {};
    var kindTag = e.kind === "tool"
      ? '<span class="tag" style="border-color:var(--c-official);color:var(--c-official)">内置能力</span>'
      : '<span class="tag on" style="border-color:var(--c-gw);color:var(--c-gw)">HTTP 插件</span>';
    var stTag = e.enabled ? '<span class="tag">● 启用</span>' : '<span class="tag">○ 停用</span>';
    return '<tr><td><b>' + esc(e.id) + '</b></td><td class="eco-summary">' + esc(meta.desc || meta.short_desc || "") + '</td>'
      + '<td>' + kindTag + '</td><td>' + stTag + '</td>'
      + '<td style="white-space:nowrap">'
      + (e.enabled
        ? '<button class="btn ghost" data-a="plug-op" data-op="disable" data-id="' + esc(e.id) + '">停用</button> '
        : '<button class="btn ghost" data-a="plug-op" data-op="enable" data-id="' + esc(e.id) + '">启用</button> ')
      + '<button class="btn ghost" data-a="plug-op" data-op="remove" data-id="' + esc(e.id) + '">卸载</button></td></tr>';
  }).join("");
  html += '<section class="card" style="margin-top:12px"><h2>🔌 已安装插件与能力</h2>'
    + '<table class="mtable"><thead><tr><th style="width:22%">条目</th><th>说明</th><th style="width:11%">类型</th><th style="width:9%">状态</th><th style="width:20%">操作</th></tr></thead>'
    + (rows ? '<tbody>' + rows + '</tbody>' : '<tbody><tr><td colspan="5" class="sub" style="padding:14px 4px">还没有安装;从下方市场一键安装。</td></tr></tbody>')
    + '</table>'
    + '<div class="sub" style="margin-top:8px">调用入口:POST /api/plugins/:id/invoke;错误一律人话 JSON(human 字段)。卸载后可随时从市场重装。</div>'
    + '</section>';
  /* 官方源市场卡片 */
  var cards = ((mk.official && mk.official.plugins) || []).map(function (p) {
    var inst = plugInstalled(p.id);
    return '<div class="eco-card"><div class="ic">' + (p.builtin ? "⚙" : "🔌") + '</div><div style="min-width:0">'
      + '<b>' + esc(p.name) + '</b><span class="d">' + esc(p.short_desc || p.desc || "") + '</span></div>'
      + (inst ? '<span class="sub" style="font-size:10px;flex:none">已安装</span>'
        : '<button class="eco-install" data-a="plug-install" data-src="official" data-id="' + esc(p.id) + '">点击安装</button>')
      + '</div>';
  }).join("");
  html += '<section class="card" style="margin-top:12px"><h2>插件市场 · 官方源</h2>'
    + '<div class="sub" style="margin:4px 0 10px">官方内置能力(识图 / 文生图 / 图编辑 / 视频抽帧)与官方插件;文生图/图编辑需上游开通图生成权限(未开通时人话提示)。</div>'
    + '<div class="eco-grid">' + (cards || '<span class="sub">暂无条目</span>') + '</div></section>';
  /* 第三方源管理 */
  var srcs = (mk.sources || []).filter(function (x) { return !x.builtin; });
  var srcRows = srcs.map(function (x) {
    return '<tr><td><b>' + esc(x.id) + '</b></td><td class="eco-summary">' + esc(x.name || "") + '</td>'
      + '<td style="white-space:nowrap">'
      + '<button class="btn ghost" data-a="plug-src-browse" data-id="' + esc(x.id) + '" data-name="' + esc(x.name || x.id) + '">浏览</button> '
      + '<button class="btn ghost" data-a="plug-src-del" data-id="' + esc(x.id) + '">移除</button></td></tr>';
  }).join("");
  var srcForm = PLUG_STATE.srcForm
    ? '<div class="grid2" style="margin-top:8px">'
      + '<div class="f"><label>源 id *</label><input class="mono" data-pk="src-id" placeholder="my-source"></div>'
      + '<div class="f"><label>名称 *</label><input data-pk="src-name" placeholder="我的插件源"></div>'
      + '<div class="f" style="grid-column:1/-1"><label>清单地址 * (http(s) 静态 JSON)</label><input class="mono" data-pk="src-url" placeholder="https://example.com/plugins.json"></div></div>'
      + '<div class="btn-row" style="margin-top:8px">'
      + '<button class="btn primary" data-a="plug-src-do">添加</button>'
      + '<button class="btn ghost" data-a="plug-src-toggle">取消</button></div>'
    : '<button class="btn ghost" data-a="plug-src-toggle">＋ 添加第三方源</button>';
  html += '<section class="card" style="margin-top:12px"><h2>第三方源</h2>'
    + '<div class="sub" style="margin:4px 0 10px">仅 http(s) 静态 JSON 清单(M5 定案);schema 不识别整源拒收,清单只读不自动执行。</div>'
    + (srcRows ? '<table class="mtable"><tbody>' + srcRows + '</tbody></table>' : '<div class="sub" style="margin-bottom:8px">还没有第三方源。</div>')
    + srcForm + '</section>';
  /* 浏览某源的插件清单 */
  var br = PLUG_STATE.browse;
  if (br) {
    html += '<section class="card" style="margin-top:12px"><h2>源 ' + esc(br.name) + ' · 插件清单</h2>';
    if (br.error) html += '<div style="padding:10px 12px;background:rgba(226,88,78,.08);border:1px solid rgba(226,88,78,.4);border-radius:8px;font-size:11.5px;color:#FFBAB4">' + esc(br.error) + '</div>';
    else if (br.loading) html += '<div class="sub" style="padding:14px 0;text-align:center">加载中…</div>';
    else {
      var brCards = (br.plugins || []).map(function (p) {
        var inst = plugInstalled(p.id);
        return '<div class="eco-card"><div class="ic">🔌</div><div style="min-width:0">'
          + '<b>' + esc(p.name || p.id) + '</b><span class="d">' + esc(p.short_desc || p.desc || "") + '</span></div>'
          + (inst ? '<span class="sub" style="font-size:10px;flex:none">已安装</span>'
            : '<button class="eco-install" data-a="plug-install" data-src="' + esc(br.sourceId) + '" data-id="' + esc(p.id) + '">安装</button>')
          + '</div>';
      }).join("");
      html += '<div class="eco-grid">' + (brCards || '<span class="sub">该源无插件条目</span>') + '</div>'
        + '<div class="btn-row" style="margin-top:10px"><button class="btn ghost" data-a="plug-src-browse-close">收起</button></div>';
    }
    html += '</section>';
  }
  return html;
}

/* ══════════════════════════════════════════════════════════════════
   插件与能力市场 v3(演示定稿)· ai-gateway 路由插件
   ──────────────────────────────────────────────────────────────
   · 插件 = 多媒体能力路由:调用上游模型能力(识图/文生图/图编辑/ASR/TTS)
     或本机工具(ffmpeg 抽帧);配置自己的端点即可用(自备端点,不依赖内置上游)
   · 市场 / 文档页(独立) / 配置页(独立) / 插件自带 UI 界面 / switch 启停 / 更新 / 卸载
   · 故障转移:多备用模型,主模型失败自动切换
   · 数据接真 API(GET /api/plugin-market + /api/plugins + /api/plugins/:id,
     契约见《插件市场-开发文档-v3》§四);batch A 未合入时按契约形状检测失败
     → 回退内置 PLUG2_FALLBACK(演示数据),保证可独立验证
   ══════════════════════════════════════════════════════════════════ */
var PLUG2 = { tab: "market", q: "", cat: "all", detail: null, dt: "doc", ui: null,
  mode: "mock", loading: false, busy: null,
  data: [], installed: {}, enabled: {}, updates: {},
  local: [], mySources: [], srcForm: false, cfgModels: null, cfgVals: null, cfgFailover: undefined };
var PLUG2_FALLBACK = [
  { id: "image-describe", name: "识图 · 图片转文字", author: "官方", version: "2.0.1", cap: "识图", icon: "🖼️", ui: true,
    mount: "media_parse",
    desc: "上传/引用一张图片,调用上游 chat 模型带图识别,输出文字描述。",
    power: "调用上游 chat 通道(图片以 image_url 注入),模型默认取供应商当前默认,可指定",
    input: ["图片(暂存 URL 或本机路径)", "识别指令(可选)"], output: ["文字描述(文本)"],
    tags: ["识图", "图片 → 文字"], status: "ready",
    scenes: [
      { title: "给非多模态模型装上「眼睛」", desc: "DeepSeek、GLM 等纯文本模型不能直接接收图片。本插件先把图片转成文字描述,再喂给模型——模型即可回答图片相关问题(截图理解、图表解读、商品图描述、验证码识别),原本非多模态的模型经此链路即获得识图能力。" },
      { title: "多模态链路中转站", desc: "视频抽帧产物、设计稿、文档截图等,先经识图转成文字,再进入对话/总结/文档流程,全链路统一走文字通道,任何模型都能参与。" },
      { title: "批量图片描述", desc: "媒体暂存里的一批图片,逐一转成文字描述,用于内容归档、检索、无障碍阅读。" },
    ],
    config: [
      { k: "model", label: "识别模型", type: "select", def: "跟随供应商默认", options: ["跟随供应商默认", "gpt-5.6", "claude-fable-5"], hint: "能力探测 image_in=Yes 的模型" },
      { k: "lang", label: "输出语言", type: "select", def: "中文", options: ["中文", "English"] },
    ],
    md: "# 识图 · 图片转文字\n\n> ai-gateway 路由插件 · 官方 · 挂载点 media_parse\n\n## 功能简介\n\n上传/引用一张图片,调用上游 chat 模型带图识别,输出文字描述。\n\n## 应用场景\n\n### 给非多模态模型装上「眼睛」\n\nDeepSeek、GLM 等纯文本模型不能直接接收图片。本插件先把图片转成文字描述,再喂给模型——模型即可回答图片相关问题(截图理解、图表解读、商品图描述、验证码识别),原本非多模态的模型经此链路即获得识图能力。\n\n### 多模态链路中转站\n\n视频抽帧产物、设计稿、文档截图等,先经识图转成文字,再进入对话/总结/文档流程,全链路统一走文字通道,任何模型都能参与。\n\n### 批量图片描述\n\n媒体暂存里的一批图片,逐一转成文字描述,用于内容归档、检索、无障碍阅读。\n\n## 调用的模型能力\n\n调用上游 chat 通道(图片以 image_url 注入),模型默认取供应商当前默认,可指定。支持多模型故障转移:主模型失败自动切换备用模型。\n\n## 使用说明\n\n1. 在「供应商」里配置好支持识图的模型(能力面板可探测 image_in)\n2. 安装本插件,选择识别模型\n3. 调用入口:POST /api/plugins/:id/invoke,传入 media_url(网关暂存或本机路径)\n4. 输出文字描述,可继续喂给对话/文档流程",
    docs: "1) 在「供应商」里配置好支持识图的模型(能力面板可探测 image_in)\n2) 安装本插件,选择识别模型\n3) 调用入口:POST /api/plugins/:id/invoke,传入 media_url(网关暂存或本机路径)\n4) 输出文字描述,可继续喂给对话/文档流程" },
  { id: "image-generate", name: "文生图 · 文字生成图片", author: "官方", version: "2.0.0", cap: "文生图", icon: "🎨", ui: true,
    mount: "tool_exec",
    desc: "输入一句话描述,调用上游 /v1/images/generations 生成图片,产物自动存入媒体暂存。",
    power: "调用上游 /v1/images/generations(gpt-image 系模型),产物 b64_json/url 双形态入暂存",
    input: ["文字描述", "尺寸(可选)"], output: ["图片(暂存 URL)"],
    tags: ["文生图", "文字 → 图片"], status: "ready",
    scenes: [
      { title: "文案配图", desc: "写文章、推文、文档时,一句话生成配图,不用再找图库。" },
      { title: "设计素材", desc: "快速生成示意图、插画、概念图,给设计/汇报提供视觉参考。" },
      { title: "创意发散", desc: "同一描述生成多张方案,辅助头脑风暴与方案比选。" },
    ],
    config: [
      { k: "model", label: "生成模型", type: "select", def: "gpt-image-1", options: ["gpt-image-1"] },
      { k: "size", label: "默认尺寸", type: "select", def: "1024×1024", options: ["1024×1024", "512×512"] },
      { k: "n", label: "一次生成张数", type: "number", def: 1 },
    ],
    md: "# 文生图 · 文字生成图片\n\n> ai-gateway 路由插件 · 官方 · 挂载点 tool_exec\n\n## 功能简介\n\n输入一句话描述,调用上游 /v1/images/generations 生成图片,产物自动存入媒体暂存。\n\n## 应用场景\n\n### 文案配图\n\n写文章、推文、文档时,一句话生成配图,不用再找图库。\n\n### 设计素材\n\n快速生成示意图、插画、概念图,给设计/汇报提供视觉参考。\n\n### 创意发散\n\n同一描述生成多张方案,辅助头脑风暴与方案比选。\n\n## 调用的模型能力\n\n调用上游 /v1/images/generations(gpt-image 系模型),产物 b64_json/url 双形态入暂存。支持多模型故障转移。\n\n## 使用说明\n\n1. 需上游 Key 组开通图生成权限(未开通时调用返回人话提示)\n2. 生成产物自动入媒体暂存,返回管内 URL\n3. 可继续喂给识图/对话流程",
    docs: "1) 需上游 Key 组开通图生成权限(未开通时调用返回人话提示)\n2) 生成产物自动入媒体暂存,返回管内 URL\n3) 可继续喂给识图/对话流程" },
  { id: "image-edit", name: "图编辑 · 图片指令修改", author: "官方", version: "1.1.0", cap: "图编辑", icon: "✂️", ui: true,
    mount: "tool_exec",
    desc: "原图 + 修改指令,调用上游 /v1/images/edits 生成新图,产物入暂存。",
    power: "调用上游 /v1/images/edits(multipart 原图 + 指令),新图入媒体暂存",
    input: ["原图(暂存 URL)", "修改指令"], output: ["新图(暂存 URL)"],
    tags: ["图编辑"], status: "ready",
    scenes: [
      { title: "图片修改", desc: "对现有图片按指令修改:换风格、改元素、扩场景,产物入暂存继续处理。" },
      { title: "多轮迭代", desc: "生成不满意?给原图加新指令再改,配合文生图形成「生成→修改→定稿」闭环。" },
    ],
    config: [
      { k: "model", label: "编辑模型", type: "select", def: "gpt-image-1", options: ["gpt-image-1"] },
    ],
    md: "# 图编辑 · 图片指令修改\n\n> ai-gateway 路由插件 · 官方 · 挂载点 tool_exec\n\n## 功能简介\n\n原图 + 修改指令,调用上游 /v1/images/edits 生成新图,产物入暂存。\n\n## 应用场景\n\n### 图片修改\n\n对现有图片按指令修改:换风格、改元素、扩场景,产物入暂存继续处理。\n\n### 多轮迭代\n\n生成不满意?给原图加新指令再改,配合文生图形成「生成→修改→定稿」闭环。\n\n## 调用的模型能力\n\n调用上游 /v1/images/edits(multipart 原图 + 指令),新图入媒体暂存。支持多模型故障转移。\n\n## 使用说明\n\n1. 需上游 Key 组开通图生成权限\n2. 原图从网关暂存或本机路径读取\n3. 产物入暂存,可继续处理",
    docs: "1) 需上游 Key 组开通图生成权限\n2) 原图从网关暂存或本机路径读取\n3) 产物入暂存,可继续处理" },
  { id: "ffmpeg-frame-extract", name: "视频抽帧 · 视频转图片", author: "官方", version: "1.0.0", cap: "抽帧", icon: "🎞️", ui: true,
    mount: "media_parse",
    desc: "按时间点从视频抽出一帧 JPEG,喂给识图类模型(路由插件示例:本机 ffmpeg 实现)。",
    power: "调用本机 ffmpeg(-ss 定点抽帧),产物 JPEG 入媒体暂存,可喂识图管线",
    input: ["视频(暂存 URL 或本机路径)", "时间点 t", "缩放 scale(可选)"], output: ["JPEG 图片(b64 / 暂存 URL)"],
    tags: ["视频 → 图片", "喂识图"], status: "ready",
    scenes: [
      { title: "让模型「看懂视频」", desc: "视频不能直接喂给模型——先按时间点抽帧成图,再经识图插件转成文字描述,模型即可回答视频内容问题(视频问答、内容摘要、监控画面分析)。" },
      { title: "素材提取", desc: "从视频取关键帧做封面、缩略图、归档样本。" },
    ],
    config: [
      { k: "ffmpegPath", label: "ffmpeg 路径", type: "text", def: "ffmpeg", hint: "留空用 PATH 中的 ffmpeg" },
      { k: "defaultT", label: "默认时间点(秒)", type: "number", def: 0 },
    ],
    md: "# 视频抽帧 · 视频转图片\n\n> ai-gateway 路由插件 · 官方 · 挂载点 media_parse\n\n## 功能简介\n\n按时间点从视频抽出一帧 JPEG,喂给识图类模型(路由插件示例:本机 ffmpeg 实现)。\n\n## 应用场景\n\n### 让模型「看懂视频」\n\n视频不能直接喂给模型——先按时间点抽帧成图,再经识图插件转成文字描述,模型即可回答视频内容问题(视频问答、内容摘要、监控画面分析)。\n\n### 素材提取\n\n从视频取关键帧做封面、缩略图、归档样本。\n\n## 调用的模型能力\n\n调用本机 ffmpeg(-ss 定点抽帧),产物 JPEG 入媒体暂存,可喂识图管线。\n\n## 使用说明\n\n1. 本机需安装 ffmpeg(缺失时调用返回人话 E_MEDIA_FFMPEG)\n2. 输入限网关暂存(hex 校验防路径穿越)或本机绝对路径\n3. 抽帧产物可立即喂给识图插件做画面理解",
    docs: "1) 本机需安装 ffmpeg(缺失时调用返回人话 E_MEDIA_FFMPEG)\n2) 输入限网关暂存(hex 校验防路径穿越)或本机绝对路径\n3) 抽帧产物可立即喂给识图插件做画面理解" },
  { id: "asr-speech", name: "语音识别 · 音频转文字", author: "官方", version: "1.1.0", cap: "ASR", icon: "🎙️", ui: true,
    mount: "media_parse",
    desc: "上传一段音频,识别为文字。配置你自己的 ASR 服务端点即可使用(Whisper 形态,兼容 OpenAI / Azure / 中转站等)。",
    power: "调用配置的 ASR 端点 POST /v1/audio/transcriptions(Whisper 形态);支持多端点故障转移",
    input: ["音频(暂存 URL)"], output: ["文字(转录)"],
    tags: ["音频 → 文字"], status: "ready",
    scenes: [
      { title: "语音转文字", desc: "会议录音、语音消息、采访素材转成文字,进入对话/总结/记录流程。配置你自己的 ASR 端点即可用。" },
    ],
    config: [
      { k: "apiBase", label: "ASR API 地址", type: "text", def: "https://api.openai.com/v1", hint: "任意提供 ASR 的服务(OpenAI Whisper / Azure 语音 / 中转站)" },
      { k: "apiKey", label: "API Key", type: "password", req: true, hint: "你的服务商 Key" },
      { k: "model", label: "识别模型", type: "select", def: "whisper-1", options: ["whisper-1", "whisper-large-v3", "其他"] },
    ],
    md: "# 语音识别 · 音频转文字\n\n> ai-gateway 路由插件 · 官方 · 挂载点 media_parse\n\n## 功能简介\n\n上传一段音频,识别为文字。配置你自己的 ASR 服务端点即可使用(Whisper 形态,兼容 OpenAI / Azure / 各类中转站)。\n\n## 应用场景\n\n### 语音转文字\n\n会议录音、语音消息、采访素材转成文字,进入对话/总结/记录流程。\n\n## 调用的模型能力\n\n调用配置的 ASR 端点 POST /v1/audio/transcriptions(Whisper 形态);支持多端点故障转移,主端点失败自动切换备用。\n\n## 使用说明\n\n1. 配置你自己的 ASR 服务端点(API 地址 + Key + 模型)\n2. 安装后即可识别音频为文字\n3. 可配置多个备用端点,主端点失败自动切换(故障转移)\n4. 产物文字可继续喂给对话/总结流程",
    docs: "1) 配置你自己的 ASR 服务端点(API 地址 + Key + 模型)\n2) 安装后即可识别音频为文字\n3) 可配置多个备用端点,主端点失败自动切换(故障转移)\n4) 产物文字可继续喂给对话/总结流程" },
  { id: "tts-speech", name: "语音合成 · 文字转语音", author: "官方", version: "1.1.0", cap: "TTS", icon: "🔊", ui: true,
    mount: "tool_exec",
    desc: "输入文字,合成为语音。配置你自己的 TTS 服务端点即可使用,支持多音色。",
    power: "调用配置的 TTS 端点 POST /v1/audio/speech;支持多端点故障转移",
    input: ["文字"], output: ["音频(暂存 URL)"],
    tags: ["文字 → 音频"], status: "ready",
    scenes: [
      { title: "文字转语音", desc: "文章、消息、通知转成语音播放,朗读与播报场景。配置你自己的 TTS 端点即可用。" },
    ],
    config: [
      { k: "apiBase", label: "TTS API 地址", type: "text", def: "https://api.openai.com/v1", hint: "任意提供 TTS 的服务(OpenAI / Azure / 中转站)" },
      { k: "apiKey", label: "API Key", type: "password", req: true, hint: "你的服务商 Key" },
      { k: "voice", label: "默认音色", type: "select", def: "alloy", options: ["alloy", "echo", "fable", "onyx", "nova", "shimmer"] },
    ],
    md: "# 语音合成 · 文字转语音\n\n> ai-gateway 路由插件 · 官方 · 挂载点 tool_exec\n\n## 功能简介\n\n输入文字,合成为语音。配置你自己的 TTS 服务端点即可使用,支持多音色。\n\n## 应用场景\n\n### 文字转语音\n\n文章、消息、通知转成语音播放,朗读与播报场景。\n\n## 调用的模型能力\n\n调用配置的 TTS 端点 POST /v1/audio/speech;支持多端点故障转移,主端点失败自动切换备用。\n\n## 使用说明\n\n1. 配置你自己的 TTS 服务端点(API 地址 + Key + 音色)\n2. 安装后即可文字转语音\n3. 可配置多个备用端点,主端点失败自动切换(故障转移)\n4. 产物音频入暂存,可播放/下载",
    docs: "1) 配置你自己的 TTS 服务端点(API 地址 + Key + 音色)\n2) 安装后即可文字转语音\n3) 可配置多个备用端点,主端点失败自动切换(故障转移)\n4) 产物音频入暂存,可播放/下载" },
];


function plug2ById(id) { return PLUG2.data.filter(function (x) { return x.id === id; })[0]; }
function plug2Installed(id) { return !!PLUG2.installed[id]; }
function plug2IconById(id) {
  var ICONS = { "image-describe": "🖼️", "image-generate": "🎨", "image-edit": "✂️", "ffmpeg-frame-extract": "🎞️", "asr-speech": "🎙️", "tts-speech": "🔊" };
  return ICONS[id] || "🔌";
}
/* 条目归一:后端驼峰契约字段 → 前端统一形状(缺失字段兜默认) */
function plug2NormEntry(p) {
  p = p || {};
  return {
    id: p.id, name: p.name || p.id || "", author: p.author || "官方", version: p.version || "1.0.0",
    cap: p.cap || "", icon: p.icon || plug2IconById(p.id),
    mount: p.mount || "", desc: p.desc || p.short_desc || "", power: p.power || "",
    input: p.input || {}, output: p.output || {},
    tags: p.tags || [], scenes: p.scenes || [], config: p.config || [], models: p.models || [],
    md: p.md || "", docs: p.docs || "", ui: !!p.ui, status: p.status || ""
  };
}
/* 市场契约形状检查:batch A 新字段(cap/tags/md)缺失 = 后端未就绪 → 回退演示数据 */
function plug2NormMarket(mk) {
  var arr = (mk && mk.official && mk.official.plugins) || [];
  if (!arr.length) return null;
  var f = arr[0];
  if (!f || (f.cap === undefined && f.tags === undefined && f.md === undefined)) return null;
  return arr.map(plug2NormEntry).filter(function (x) { return !!x.id; });
}
/* 输入→输出计数:后端为 {required,properties} 对象,演示数据为数组,兼容两种 */
function plug2InOut(p) {
  var inLen = (p.input && p.input.length !== undefined) ? p.input.length : ((p.input && p.input.required) || []).length;
  var outLen = (p.output && p.output.length !== undefined) ? p.output.length : (p.output && p.output.properties ? Object.keys(p.output.properties).length : 0);
  return inLen + " → " + outLen;
}
function plug2SemverGt(a, b) {
  var pa = String(a || "").split(".").map(Number), pb = String(b || "").split(".").map(Number);
  for (var k = 0; k < 3; k++) { var x = pa[k] || 0, y = pb[k] || 0; if (x !== y) return x > y; }
  return false;
}
/* 模型与故障转移表数据:manifest models 优先,缺省按能力生成主/备用(演示形态) */
function plug2Models(p) {
  if (p.models && p.models.length) return p.models.map(function (m) {
    return { model: m.id || "", ep: (m.api || "").replace(/^https?:\/\//, ""), note: m.note || "" };
  });
  var cap = p.cap || "";
  return [
    { model: cap === "识图" ? "gpt-5.6" : (cap === "文生图" || cap === "图编辑" ? "gpt-image-1" : (cap === "ASR" ? "whisper-1" : (cap === "TTS" ? "tts-1" : "deepseek-chat"))), ep: "2xa.cc.cd /v1", note: cap === "ASR" || cap === "TTS" ? "你的主端点" : "多模态,直接识别" },
    { model: cap === "识图" ? "deepseek-chat" : (cap === "文生图" ? "dall-e-3" : (cap === "ASR" ? "whisper-large-v3" : (cap === "TTS" ? "tts-1-hd" : "glm-5"))), ep: "api.deepseek.com /v1", note: cap === "识图" ? "纯文本,经本插件文字链路识别" : "备用通道" },
    { model: "glm-5-flash", ep: "bigmodel.cn /v1", note: "低成本兜底" },
  ];
}
/* 配置页状态:models 行 / 参数值 / 故障转移开关(仅 UI 态,保存时组包提交) */
function plug2CfgState(p) {
  if (!PLUG2.cfgModels || PLUG2.cfgModels.id !== p.id) {
    PLUG2.cfgModels = { id: p.id, rows: plug2Models(p) };
    PLUG2.cfgVals = {};
    (p.config || []).forEach(function (c) {
      PLUG2.cfgVals[c.k] = (c.def !== undefined && c.def !== null) ? c.def : ((c.options && c.options.length) ? c.options[0] : "");
    });
    PLUG2.cfgFailover = true;
  }
  return PLUG2.cfgModels;
}
/* 数据加载:市场 + 已安装;市场契约不满足 → PLUG2_FALLBACK 兜底(mock 模式) */
function plug2Load() {
  PLUG2.loading = true;
  if (state.view === "plug") render();
  var m = api.plugMarket().then(plug2NormMarket).catch(function () { return null; });
  var i = api.plugList().then(function (v) {
    var arr = (v && v.plugins) || [];
    var inst = {}, en = {}, list = [];
    arr.forEach(function (x) { inst[x.id] = 1; en[x.id] = !!x.enabled; list.push(x); });
    return { inst: inst, en: en, list: list };
  }).catch(function () { return null; });
  return Promise.all([m, i]).then(function (r) {
    var mk = r[0], ii = r[1];
    if (!mk) {
      PLUG2.mode = "mock";
      PLUG2.data = PLUG2_FALLBACK;
      PLUG2.installed = { "ffmpeg-frame-extract": 1 };
      PLUG2.enabled = { "ffmpeg-frame-extract": true };
      PLUG2.updates = { "image-describe": "2.1.0", "ffmpeg-frame-extract": "1.1.0" };
    } else {
      PLUG2.mode = "api";
      PLUG2.data = mk;
      PLUG2.installed = (ii && ii.inst) || {};
      PLUG2.enabled = (ii && ii.en) || {};
      PLUG2.updates = {};
      if (ii) {
        ii.list.forEach(function (x) {
          var p = plug2ById(x.id);
          var instV = x.version || (x.meta && x.meta.version) || null;
          if (p) {
            /* 可更新 = 市场版本 > 已装版本 */
            if (instV && plug2SemverGt(p.version, instV)) PLUG2.updates[x.id] = p.version;
          } else {
            /* 本地/第三方已装条目不在市场数据里 → 补进列表 */
            var mt = x.meta || {};
            PLUG2.data.push(plug2NormEntry({ id: x.id, name: mt.name, version: instV, short_desc: mt.short_desc, desc: mt.desc, mount: mt.mount }));
          }
        });
      }
    }
    PLUG2.loading = false;
    if (state.view === "plug") render();
  });
}
function plug2OpenDetail(id, dt) {
  PLUG2.tab = "detail"; PLUG2.detail = id; PLUG2.dt = dt;
  PLUG2.cfgModels = null; PLUG2.cfgVals = null; PLUG2.cfgFailover = undefined;
  render();
  if (PLUG2.mode !== "api") return;
  api.plugDetail(id).then(function (v) {
    var e = (v && v.id) ? v : ((v && v.plugin) || null);
    if (!e) return;
    var i = PLUG2.data.findIndex(function (x) { return x.id === id; });
    if (i < 0) PLUG2.data.push(plug2NormEntry(e));
    else PLUG2.data[i] = plug2NormEntry(e);
    if (state.view === "plug" && PLUG2.tab === "detail" && PLUG2.detail === id) render();
  }).catch(function () { /* 详情失败维持市场数据 */ });
}
/* 保存配置:参数值 + models 优先级 + 故障转移开关 → PUT /api/plugins/:id/config */
function plug2SaveCfg(id) {
  var rows = (PLUG2.cfgModels && PLUG2.cfgModels.rows) || [];
  var models = rows.map(function (r) {
    var api = String(r.ep || "");
    if (api && api.indexOf("://") < 0) api = "https://" + api;
    return { id: r.model, api: api, note: r.note };
  }).filter(function (m) { return m.id; });
  var body = { config: PLUG2.cfgVals || {}, models: models, failover: PLUG2.cfgFailover !== false };
  if (PLUG2.mode !== "api") { showToast("配置已保存,立即生效", "ok"); render(); return; }
  PLUG2.busy = id;
  api.plugConfig(id, body).then(function () { showToast("配置已保存,立即生效", "ok"); })
    .catch(function (e) { showToast(e.message || "保存失败", "error"); })
    .finally(function () { PLUG2.busy = null; render(); });
}
/* 本地 manifest 文件:校验 → 上架 → 立即安装(来源 local) */
function plug2LocalFile(inp) {
  var f = inp.files && inp.files[0];
  if (!f) return;
  var rd = new FileReader();
  rd.onload = function () {
    var m;
    try { m = JSON.parse(rd.result); } catch (e) { showToast("manifest JSON 解析失败:" + e.message, "error"); return; }
    if (!m || !m.id || !m.name) { showToast("manifest 校验失败:缺 id/name", "error"); return; }
    if (PLUG2.mode !== "api") {
      PLUG2.local.push({ id: m.id, name: m.name, src: "本地", detail: f.name + " · " + (m.desc || m.short_desc || "") });
      showToast("✓ 本地插件「" + m.name + "」校验通过,已上架(演示)", "ok");
      render();
      return;
    }
    PLUG2.busy = m.id;
    api.plugLocal(m).then(function (v) {
      var rid = (v && v.id) ? v.id : ("local." + m.id);
      showToast("✓ 本地插件「" + m.name + "」校验通过,已上架", "ok");
      api.plugInstallId(rid).then(function () { showToast("已安装「" + m.name + "」", "ok"); })
        .catch(function () { /* 上架成功即可,安装失败不阻断 */ });
    }).catch(function (e) { showToast("校验失败:" + (e.message || "未知错误"), "error"); })
      .finally(function () { PLUG2.busy = null; plug2Load(); });
  };
  rd.readAsText(f);
  inp.value = "";
}

/* 迷你 markdown 渲染(# / ## / ### / - / 1. / 引用 > / 代码块)——插件文档展示用 */
function md2html(md) {
  var lines = String(md || "").split("\n");
  var html = "", inCode = false;
  lines.forEach(function (ln) {
    var t = ln.trim();
    if (t.indexOf("```") === 0) {
      if (inCode) { html += "</pre>"; inCode = false; } else { html += '<pre class="mono" style="font-size:11px;line-height:1.7;background:var(--raised);border:1px solid var(--hair);border-radius:8px;padding:9px 11px;overflow-x:auto;white-space:pre-wrap">'; inCode = true; }
      return;
    }
    if (inCode) { html += esc(ln) + "\n"; return; }
    if (/^### /.test(t)) { html += '<h4 style="font-size:12.5px;margin:12px 0 5px;color:#cdd6e6">' + esc(t.slice(4)) + '</h4>'; return; }
    if (/^## /.test(t)) { html += '<h3 style="font-size:13px;margin:16px 0 6px;color:#9CB4DE;border-left:3px solid #3d5a8a;padding-left:8px">' + esc(t.slice(3)) + '</h3>'; return; }
    if (/^# /.test(t)) { html += '<h2 style="font-size:15px;margin:0 0 4px">' + esc(t.slice(2)) + '</h2>'; return; }
    if (/^> /.test(t)) { html += '<div class="sub" style="font-size:11px;margin:2px 0 10px">' + esc(t.slice(2)) + '</div>'; return; }
    if (/^\d+\. /.test(t)) { html += '<div style="font-size:12px;line-height:1.9;padding-left:2px">' + esc(t) + '</div>'; return; }
    if (t.indexOf("- ") === 0) { html += '<div style="font-size:12px;line-height:1.9;padding-left:2px">· ' + esc(t.slice(2)) + '</div>'; return; }
    if (t === "") { html += '<div style="height:5px"></div>'; return; }
    html += '<p style="font-size:12px;line-height:1.9;margin:0">' + esc(t) + '</p>';
  });
  if (inCode) html += "</pre>";
  return html;
}

function plugCenterHtml() {
  if (PLUG2.ui) return plug2UiHtml(plug2ById(PLUG2.ui));
  if (PLUG2.loading && !PLUG2.data.length) {
    return '<section class="card"><h2>🧩 插件与能力市场</h2>'
      + '<div class="sub" style="padding:20px 0;text-align:center">加载中…</div></section>';
  }
  if (PLUG2.tab === "detail" && PLUG2.detail) return plug2DetailHtml(plug2ById(PLUG2.detail));
  var tabs = [["market", "🛒 插件市场"], ["installed", "📦 已安装"], ["design", "🧩 我的设计"]];
  var html = '<section class="card"><h2>🧩 插件与能力市场</h2>'
    + (PLUG2.mode === "mock"
      ? '<div class="sub" style="margin:2px 0 8px">⚠ 后端插件 API 未就绪(batch A 未合入),当前展示内置演示数据;后端就绪后自动切换真实数据。</div>'
      : '')
    + '<div style="margin:2px 0 12px;padding:12px 14px;background:linear-gradient(135deg,rgba(61,126,255,.08),rgba(123,92,255,.06));border:1px solid rgba(123,92,255,.3);border-radius:12px">'
    + '<b style="font-size:13px">ai-gateway 路由插件 · 给大模型赋能</b>'
    + '<div class="sub" style="margin:5px 0 0;line-height:1.8">我们的网关是模型路由中心,插件是给模型赋能的路由插件:</div>'
    + '<div class="sub" style="margin:3px 0 0;line-height:1.9">'
    + '① <b>能力补全</b>:识图、文生图、图编辑、视频抽帧、语音——让纯文本模型也能看图、让任何模型都能产出图片与视频理解;<br>'
    + '② <b>故障转移</b>:每个能力可配置多个备用模型,主模型调用失败自动切换下一个,服务不断;<br>'
    + '③ <b>开放生态</b>:manifest 声明即插件(挂载点 / 输入输出 / 配置项),任何人都能设计发布,别人一键安装;<br>'
    + '④ <b>自备端点</b>:ASR / TTS 等能力配置你自己的服务端点即可使用(不依赖内置上游,谁有大模型谁配置)。</div></div>'
    + '<div class="eco-tabs" style="margin-bottom:12px">'
    + tabs.map(function (t) { return '<button class="eco-tab ' + (PLUG2.tab === t[0] ? "on" : "") + '" data-a="plug2-tab" data-t="' + t[0] + '">' + t[1] + '</button>'; }).join("")
    + '</div>'
    + (PLUG2.tab === "market" ? plug2MarketHtml() : PLUG2.tab === "installed" ? plug2InstalledHtml() : plug2DesignHtml())
    + '</section>';
  return html;
}

/* ── 市场页:搜索 / 分类 / 列表(行 = 图标+名称+作者·版本·挂载点 | 能力 | 输入→输出 | 标签 | 状态 | 操作)── */
function plug2MarketHtml() {
  var cats = ["all", "识图", "文生图", "图编辑", "抽帧", "语音"];
  var list = PLUG2.data.filter(function (p) {
    if (PLUG2.cat !== "all" && p.cap !== PLUG2.cat) return false;
    if (PLUG2.q && (p.name + p.desc + (p.tags || []).join(" ")).toLowerCase().indexOf(PLUG2.q.toLowerCase()) < 0) return false;
    return true;
  });
  var html = '<div style="display:flex;gap:8px;flex-wrap:wrap;align-items:center;margin-bottom:12px">'
    + '<input data-a="plug2-search" placeholder="搜索插件(识图 / 生图 / 抽帧…)" value="' + esc(PLUG2.q) + '" style="flex:1;min-width:180px;background:var(--raised);border:1px solid var(--hair);border-radius:8px;color:var(--text);padding:8px 12px;font-size:12px">'
    + cats.map(function (c) { return '<button class="eco-tab ' + (PLUG2.cat === c ? "on" : "") + '" data-a="plug2-cat" data-v="' + c + '">' + (c === "all" ? "全部" : c) + '</button>'; }).join("")
    + '</div>';
  if (!list.length) return html + '<div class="sub" style="padding:30px 0;text-align:center">没有匹配的插件;换个关键词,或到「我的设计」自己做一个。</div>';
  html += '<table class="mtable"><thead><tr><th style="width:36%">插件</th><th style="width:10%">能力</th><th style="width:12%">输入 → 输出</th><th>标签</th><th style="width:11%">状态</th><th style="width:12%">操作</th></tr></thead><tbody>'
    + list.map(plug2Row).join("") + '</tbody></table>';
  return html;
}
function plug2Row(p) {
  var kindTag = p.cap ? '<span class="tag on" style="border-color:var(--c-gw);color:var(--c-gw)">' + esc(p.cap) + '</span>' : '<span class="tag">工具</span>';
  var inst = plug2Installed(p.id);
  var st = (inst ? '<span class="tag" style="border-color:var(--c-official);color:var(--c-official)">● 已启用</span>' : '<span class="tag">未安装</span>');
  return '<tr style="cursor:pointer" data-a="plug2-open" data-id="' + esc(p.id) + '">'
    + '<td><div style="display:flex;align-items:center;gap:9px">'
    + '<span style="width:30px;height:30px;border-radius:8px;background:var(--raised);display:flex;align-items:center;justify-content:center;font-size:15px;flex:none">' + p.icon + '</span>'
    + '<div style="min-width:0"><b style="font-size:12.5px">' + esc(p.name) + '</b>'
    + '<span class="sub" style="font-size:10px;display:block">' + esc(p.author) + ' · v' + esc(p.version) + ' · 挂载 ' + esc(p.mount) + '</span></div></div></td>'
    + '<td>' + kindTag + '</td>'
    + '<td>' + plug2InOut(p) + '</td>'
    + '<td>' + (p.tags || []).map(function (t) { return '<span class="tag">' + esc(t) + '</span>'; }).join("") + '</td>'
    + '<td>' + st + '</td>'
    + '<td style="white-space:nowrap">'
    + (inst
      ? '<button class="btn sm ghost" data-a="plug2-open" data-id="' + esc(p.id) + '" style="color:var(--c-official)">配置</button>'
      : '<button class="btn sm primary" data-a="plug2-install" data-id="' + esc(p.id) + '">安装</button>')
    + '</td></tr>';
}

/* ── 详情:横幅 + 文档页(独立) / 配置页(独立) ── */
function plug2DetailHtml(p) {
  if (!p) return '<div class="sub">插件不存在</div>';
  var inst = plug2Installed(p.id);
  var on = !!PLUG2.enabled[p.id];
  var hasUpdate = !!PLUG2.updates[p.id];
  var html = '<div style="display:flex;gap:10px;align-items:center;margin-bottom:14px">'
    + '<button class="btn ghost" data-a="plug2-back">← 返回</button>'
    + '<span class="sub" style="margin:0">插件详情</span></div>'
    + '<section class="card" style="background:linear-gradient(135deg,rgba(61,126,255,.07),rgba(123,92,255,.05))">'
    + '<div style="display:flex;gap:14px;align-items:flex-start">'
    + '<span style="width:52px;height:52px;border-radius:13px;background:var(--raised);display:flex;align-items:center;justify-content:center;font-size:26px;flex:none">' + p.icon + '</span>'
    + '<div style="flex:1;min-width:0"><h2 style="margin:0 0 3px;font-size:16px">' + esc(p.name) + '</h2>'
    + '<div class="sub" style="margin:0 0 6px">' + esc(p.author) + ' · v' + esc(p.version) + (hasUpdate ? ' · <span class="tag on" style="border-color:#E8A84A;color:#E8A84A">可更新 → v' + esc(PLUG2.updates[p.id]) + '</span>' : '') + '</div>'
    + '<div style="display:flex;gap:4px;flex-wrap:wrap">' + (p.tags || []).map(function (t) { return '<span class="tag">' + esc(t) + '</span>'; }).join("")
    + (p.cap ? '<span class="tag on" style="border-color:var(--c-gw);color:var(--c-gw)">' + esc(p.cap) + '</span>' : '') + '</div></div>'
    + (!inst
      ? '<button class="btn primary" data-a="plug2-install" data-id="' + esc(p.id) + '" style="flex:none">安装</button>'
      : '<div style="display:flex;gap:6px;flex:none">'
        + (p.ui ? '<button class="plug-ic" data-tip="打开插件界面" data-a="plug2-ui" data-id="' + esc(p.id) + '">🖥️</button>' : '')
        + '<button class="plug-ic ' + (PLUG2.dt === "doc" ? "on" : "") + '" data-tip="文档" data-a="plug2-doc" data-id="' + esc(p.id) + '">📖</button>'
        + '<button class="plug-ic ' + (PLUG2.dt === "cfg" ? "on" : "") + '" data-tip="配置" data-a="plug2-open" data-id="' + esc(p.id) + '">⚙️</button></div>')
    + '</div></section>';
  if (PLUG2.dt === "cfg" && inst) {
    /* 配置页(独立):模型与故障转移 + 参数 + 启停/更新/卸载 */
    var cs = plug2CfgState(p);
    html += '<section class="card" style="margin-top:12px"><h2>模型与故障转移</h2>'
      + '<div style="display:flex;align-items:center;gap:8px;margin:8px 0">'
      + '<input type="checkbox" data-pk="failover"' + (PLUG2.cfgFailover !== false ? " checked" : "") + ' style="accent-color:var(--c-gw)"><span style="font-size:12px">启用故障转移</span>'
      + '<span class="sub" style="font-size:10.5px;margin:0">主模型失败(超时/限流/配额/接口错误)自动切换下一个备用模型,服务不断</span></div>'
      + '<table class="mtable"><thead><tr><th style="width:9%">优先级</th><th>模型</th><th style="width:20%">API 端点</th><th style="width:30%">说明</th><th style="width:12%">操作</th></tr></thead><tbody>'
      + cs.rows.map(function (r, i) {
        return '<tr><td><span class="tag ' + (i === 0 ? 'on" style="border-color:var(--c-gw);color:var(--c-gw)' : '"') + '>' + (i === 0 ? "主" : "备用 " + i) + '</span></td>'
          + '<td><input class="mono" data-pk="m-' + i + '-model" value="' + esc(r.model) + '" placeholder="模型 id" style="width:100%;background:transparent;border:1px solid var(--hair);border-radius:6px;padding:2px 6px;font-size:11px"></td>'
          + '<td><input class="mono" data-pk="m-' + i + '-ep" value="' + esc(r.ep) + '" placeholder="api.deepseek.com /v1" style="width:100%;background:transparent;border:1px solid var(--hair);border-radius:6px;padding:2px 6px;font-size:10.5px"></td>'
          + '<td><input class="mono" data-pk="m-' + i + '-note" value="' + esc(r.note) + '" placeholder="说明(可选)" style="width:100%;background:transparent;border:1px solid var(--hair);border-radius:6px;padding:2px 6px;font-size:10.5px"></td>'
          + '<td style="white-space:nowrap">' + (i ? '<button class="btn sm ghost" data-a="plug2-mup" data-i="' + i + '">上移</button> ' : '') + '<button class="btn sm ghost" data-a="plug2-mdel" data-i="' + i + '">移除</button></td></tr>';
      }).join("")
      + '</tbody></table>'
      + '<div class="btn-row" style="margin-top:8px"><button class="btn ghost" data-a="plug2-madd">＋ 添加备用模型</button></div></section>';
    if ((p.config || []).length) {
      html += '<section class="card" style="margin-top:12px"><h2>参数配置</h2>'
        + '<div class="grid2" style="margin-top:6px">'
        + (p.config || []).map(function (c) {
          var v = (PLUG2.cfgVals && PLUG2.cfgVals[c.k] !== undefined) ? PLUG2.cfgVals[c.k] : "";
          var ctrl = "";
          if (c.type === "password") ctrl = '<input class="mono" type="password" data-pk="c-' + esc(c.k) + '" placeholder="••••••••" value="' + esc(String(v)) + '" style="width:100%">';
          else if (c.type === "select") ctrl = '<select class="mono" data-pk="c-' + esc(c.k) + '" style="width:100%;background:var(--raised);border:1px solid var(--hair);border-radius:7px;color:var(--text);padding:6px 9px">' + (c.options || []).map(function (o) { return '<option' + (String(o) === String(v) ? " selected" : "") + '>' + esc(o) + '</option>'; }).join("") + '</select>';
          else if (c.type === "number") ctrl = '<input class="mono" type="number" data-pk="c-' + esc(c.k) + '" value="' + esc(String(v)) + '" style="width:100%">';
          else if (c.type === "toggle") ctrl = '<input type="checkbox" data-pk="c-' + esc(c.k) + '"' + (v ? " checked" : "") + ' style="accent-color:var(--c-gw)">';
          else ctrl = '<input class="mono" data-pk="c-' + esc(c.k) + '" value="' + esc(String(v)) + '" style="width:100%">';
          return '<div class="f" style="grid-column:1/-1"><label>' + esc(c.label) + (c.req ? ' <span style="color:var(--c-err)">*</span>' : '') + '</label>' + ctrl
            + (c.hint ? '<span class="sub" style="font-size:10px">' + esc(c.hint) + '</span>' : '') + '</div>';
        }).join("")
        + '</div>'
        + '<div class="btn-row" style="margin-top:10px"><button class="btn primary" data-a="plug2-save">保存配置</button></div></section>';
    }
    html += '<div style="display:flex;align-items:center;gap:8px;margin-top:14px">'
      + '<span class="switch ' + (on ? "on" : "") + '" data-a="plug2-toggle" data-id="' + esc(p.id) + '" data-tip="' + (on ? "已打开 · 点击停止" : "已关闭 · 点击启用") + '"></span>'
      + (hasUpdate ? '<button class="btn primary" data-a="plug2-update" data-id="' + esc(p.id) + '">更新到 v' + esc(PLUG2.updates[p.id]) + '</button>' : '')
      + '<button class="btn ghost" data-a="plug2-uninstall" data-id="' + esc(p.id) + '">卸载</button></div>';
  } else {
    /* 文档页(独立):markdown 渲染 + 未安装时安装按钮 */
    html += '<section class="card" style="margin-top:12px">' + md2html(p.md || (p.docs || "")) + '</section>';
    if (!inst) {
      html += '<div class="btn-row" style="margin-top:12px"><button class="btn primary" data-a="plug2-install" data-id="' + esc(p.id) + '">安装</button></div>';
    }
  }
  return html;
}

/* ── 已安装(AstrBot 风格:行 + switch + 图标按钮,同排)── */
function plug2InstalledHtml() {
  var rows = PLUG2.data.filter(function (p) { return plug2Installed(p.id); });
  if (!rows.length) return '<div class="sub" style="padding:26px 0;text-align:center">还没有安装插件;到「插件市场」挑一个媒体能力插件装上,配置后即可使用。</div>';
  return rows.map(function (p) {
    var on = !!PLUG2.enabled[p.id];
    var hasUpdate = !!PLUG2.updates[p.id];
    return '<div style="display:flex;align-items:center;gap:12px;background:var(--raised);border:1px solid var(--hair);border-radius:12px;padding:12px 14px;margin-bottom:8px">'
      + '<span style="width:36px;height:36px;border-radius:9px;background:#1d2640;display:flex;align-items:center;justify-content:center;font-size:18px;flex:none">' + p.icon + '</span>'
      + '<div style="flex:1;min-width:0">'
      + '<div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap">'
      + '<b style="font-size:13px">' + esc(p.name) + '</b>'
      + '<span class="tag" style="font-size:10px">v' + esc(p.version) + '</span>'
      + (p.cap ? '<span class="tag on" style="border-color:var(--c-gw);color:var(--c-gw);font-size:10px">' + esc(p.cap) + '</span>' : '')
      + (hasUpdate ? '<span class="tag" style="border-color:#E8A84A;color:#E8A84A;font-size:10px">可更新 v' + esc(PLUG2.updates[p.id]) + '</span>' : '')
      + '</div>'
      + '<div class="sub" style="font-size:11px;margin-top:3px;line-height:1.6">' + esc(p.desc) + '</div></div>'
      + '<div style="display:flex;align-items:center;gap:5px;flex:none">'
      + '<span class="switch ' + (on ? "on" : "") + '" data-a="plug2-toggle" data-id="' + esc(p.id) + '" data-tip="' + (on ? "已打开 · 点击停止" : "已关闭 · 点击启用") + '"></span>'
      + (p.ui ? '<button class="plug-ic" data-tip="打开插件界面" data-a="plug2-ui" data-id="' + esc(p.id) + '">🖥️</button>' : '')
      + '<button class="plug-ic" data-tip="配置" data-a="plug2-open" data-id="' + esc(p.id) + '">⚙️</button>'
      + '<button class="plug-ic" data-tip="文档" data-a="plug2-doc" data-id="' + esc(p.id) + '">📖</button>'
      + (hasUpdate ? '<button class="plug-ic" style="border-color:#E8A84A;color:#E8A84A" data-tip="更新到 v' + esc(PLUG2.updates[p.id]) + '" data-a="plug2-update" data-id="' + esc(p.id) + '">⟳</button>' : '')
      + '<button class="plug-ic" style="color:var(--c-err)" data-tip="卸载" data-a="plug2-uninstall" data-id="' + esc(p.id) + '">🗑</button></div>'
      + '</div>';
  }).join("");
}

/* ── 插件自带 UI 界面(每个插件自己的操作台)── */
function plug2UiHtml(p) {
  if (!p) return '<div class="sub">插件不存在</div>';
  var html = '<div style="display:flex;gap:10px;align-items:center;margin-bottom:14px">'
    + '<button class="btn ghost" data-a="plug2-ui-back">← 返回</button>'
    + '<span class="sub" style="margin:0">' + esc(p.name) + ' · 界面</span></div>'
    + '<section class="card">'
    + '<div style="display:flex;gap:10px;align-items:center;margin-bottom:12px">'
    + '<span style="width:38px;height:38px;border-radius:10px;background:var(--raised);display:flex;align-items:center;justify-content:center;font-size:19px">' + p.icon + '</span>'
    + '<div><h2 style="font-size:14px;margin:0">' + esc(p.name) + '</h2>'
    + '<span class="sub" style="font-size:11px">插件自带操作界面 · 直接使用本插件能力</span></div></div>';
  if (p.cap === "识图") {
    html += '<div class="f"><label>图片(网关暂存 URL 或本机路径)</label><input class="mono" data-pk="ui-img" placeholder="http://127.0.0.1:8787/media/ab12….jpg 或 /path/to/image.png" value="http://127.0.0.1:8787/media/demo-64x64.png" style="width:100%"></div>'
      + '<div class="f" style="margin-top:8px"><label>识别指令(可选)</label><input class="mono" data-pk="ui-q" placeholder="描述这张图片的内容" value="描述这张图片的内容" style="width:100%"></div>'
      + '<div class="btn-row" style="margin-top:10px"><button class="btn primary" data-a="plug2-ui-run" data-id="' + esc(p.id) + '">▶ 识别</button></div>'
      + '<div style="margin-top:12px;padding:12px 14px;background:var(--raised);border:1px solid var(--hair);border-radius:10px;min-height:70px">'
      + '<div class="sub" style="font-size:10px;margin-bottom:6px">识别结果</div>'
      + '<div id="uiResult" style="font-size:12px;line-height:1.8;color:#A9E6C3">识别使用主模型 gpt-5.6(多模态直接识别);失败自动切换备用 deepseek-chat(文字链路)。</div></div>';
  } else if (p.cap === "文生图") {
    html += '<div class="f"><label>画面描述</label><input class="mono" data-pk="ui-q" placeholder="一只在星空下奔跑的机械狐狸…" value="一只在星空下奔跑的机械狐狸,赛博朋克风格" style="width:100%"></div>'
      + '<div class="grid2" style="margin-top:8px"><div class="f"><label>尺寸</label><select class="mono" style="width:100%;background:var(--raised);border:1px solid var(--hair);border-radius:7px;color:var(--text);padding:6px 9px"><option selected>1024×1024</option><option>512×512</option></select></div>'
      + '<div class="f"><label>张数</label><input class="mono" type="number" value="1" style="width:100%"></div></div>'
      + '<div class="btn-row" style="margin-top:10px"><button class="btn primary" data-a="plug2-ui-run" data-id="' + esc(p.id) + '">▶ 生成</button></div>'
      + '<div style="margin-top:12px;padding:12px 14px;background:var(--raised);border:1px solid var(--hair);border-radius:10px;min-height:70px">'
      + '<div class="sub" style="font-size:10px;margin-bottom:6px">生成结果(产物自动入媒体暂存)</div>'
      + '<div id="uiResult" style="font-size:12px;line-height:1.8;color:#A9E6C3">生成使用 gpt-image-1;产物返回管内 URL,可继续喂识图/对话流程。</div></div>';
  } else if (p.cap === "图编辑") {
    html += '<div class="f"><label>原图(暂存 URL)</label><input class="mono" data-pk="ui-img" placeholder="http://127.0.0.1:8787/media/….jpg" style="width:100%"></div>'
      + '<div class="f" style="margin-top:8px"><label>修改指令</label><input class="mono" data-pk="ui-q" placeholder="把背景换成海边日落" value="把背景换成海边日落" style="width:100%"></div>'
      + '<div class="btn-row" style="margin-top:10px"><button class="btn primary" data-a="plug2-ui-run" data-id="' + esc(p.id) + '">▶ 编辑</button></div>'
      + '<div style="margin-top:12px;padding:12px 14px;background:var(--raised);border:1px solid var(--hair);border-radius:10px;min-height:70px">'
      + '<div class="sub" style="font-size:10px;margin-bottom:6px">编辑结果(新图入暂存)</div>'
      + '<div id="uiResult" style="font-size:12px;line-height:1.8;color:#A9E6C3">编辑使用 gpt-image-1;新图入媒体暂存,可继续处理。</div></div>';
  } else if (p.cap === "抽帧") {
    html += '<div class="f"><label>视频(暂存 URL 或本机路径)</label><input class="mono" data-pk="ui-video" placeholder="http://127.0.0.1:8787/media/….mp4 或 /path/to/video.mp4" value="http://127.0.0.1:8787/media/demo.mp4" style="width:100%"></div>'
      + '<div class="grid2" style="margin-top:8px"><div class="f"><label>时间点 t(秒)</label><input class="mono" type="number" value="0.5" style="width:100%"></div>'
      + '<div class="f"><label>缩放(宽)</label><input class="mono" type="number" value="480" style="width:100%"></div></div>'
      + '<div class="btn-row" style="margin-top:10px"><button class="btn primary" data-a="plug2-ui-run" data-id="' + esc(p.id) + '">▶ 抽帧</button></div>'
      + '<div style="margin-top:12px;padding:12px 14px;background:var(--raised);border:1px solid var(--hair);border-radius:10px;min-height:70px">'
      + '<div class="sub" style="font-size:10px;margin-bottom:6px">抽帧结果(JPEG 入暂存)</div>'
      + '<div id="uiResult" style="font-size:12px;line-height:1.8;color:#A9E6C3">抽帧使用本机 ffmpeg;产物 JPEG 可立即喂给识图插件做画面理解。</div></div>';
  } else if (p.cap === "ASR") {
    html += '<div class="f"><label>音频(暂存 URL)</label><input class="mono" data-pk="ui-audio" placeholder="http://127.0.0.1:8787/media/….mp3" value="http://127.0.0.1:8787/media/demo.mp3" style="width:100%"></div>'
      + '<div class="btn-row" style="margin-top:10px"><button class="btn primary" data-a="plug2-ui-run" data-id="' + esc(p.id) + '">▶ 识别</button></div>'
      + '<div style="margin-top:12px;padding:12px 14px;background:var(--raised);border:1px solid var(--hair);border-radius:10px;min-height:70px">'
      + '<div class="sub" style="font-size:10px;margin-bottom:6px">转录结果</div>'
      + '<div id="uiResult" style="font-size:12px;line-height:1.8;color:#A9E6C3">识别使用你配置的 ASR 端点(whisper-1);失败自动切换备用端点。</div></div>';
  } else if (p.cap === "TTS") {
    html += '<div class="f"><label>要合成的文字</label><input class="mono" data-pk="ui-text" placeholder="你好,欢迎使用 ai-gateway" value="你好,欢迎使用 ai-gateway" style="width:100%"></div>'
      + '<div class="btn-row" style="margin-top:10px"><button class="btn primary" data-a="plug2-ui-run" data-id="' + esc(p.id) + '">▶ 合成</button></div>'
      + '<div style="margin-top:12px;padding:12px 14px;background:var(--raised);border:1px solid var(--hair);border-radius:10px;min-height:70px">'
      + '<div class="sub" style="font-size:10px;margin-bottom:6px">合成结果(音频入暂存)</div>'
      + '<div id="uiResult" style="font-size:12px;line-height:1.8;color:#A9E6C3">合成使用你配置的 TTS 端点;产物音频入暂存,可播放/下载。</div></div>';
  }
  html += '</section>';
  return html;
}

/* ── 我的设计(开放生态:任何人设计插件)── */
function plug2DesignHtml() {
  var mine = PLUG2.local.concat(PLUG2.mySources);
  var html = '<div class="sub" style="line-height:1.9">插件由 AI 或其他工具设计生成(一份 manifest:挂载点 / 输入输出 / 配置项 / 模型),<b>通过本地添加 / 粘贴 / 远程清单接入</b>——我们提供开放可拓展的插件市场能力,不内置生成器。上架后别人可安装使用。</div>'
    /* 三种接入方式 */
    + '<div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(240px,1fr));gap:10px;margin-top:12px">'
    /* ① 本地文件 */
    + '<div style="background:var(--raised);border:1px solid var(--hair);border-radius:12px;padding:12px 13px">'
    + '<b style="font-size:12.5px">📁 本地添加(推荐)</b>'
    + '<div class="sub" style="font-size:10.5px;margin:4px 0 8px;line-height:1.7">选择本机的 manifest JSON 文件(或插件目录),校验通过即本地上架,立即安装使用。不依赖任何服务器。</div>'
    + '<input type="file" accept=".json" data-a="plug2-local-file" style="font-size:11px;color:var(--muted)">'
    + '<div class="sub" style="font-size:10px;margin-top:6px">支持:manifest.json / 打包 zip / 插件目录</div></div>'
    /* ② 粘贴 manifest */
    + '<div style="background:var(--raised);border:1px solid var(--hair);border-radius:12px;padding:12px 13px">'
    + '<b style="font-size:12.5px">📋 粘贴 manifest</b>'
    + '<div class="sub" style="font-size:10.5px;margin:4px 0 8px;line-height:1.7">用 AI 或工具设计好的 manifest JSON,直接粘贴校验添加。</div>'
    + '<textarea class="mono" data-pk="d-json" rows="4" placeholder="{ &quot;id&quot;: &quot;my-plugin&quot;, &quot;mount&quot;: &quot;media_parse&quot;, &quot;models&quot;: [...], &quot;config&quot;: [...] }" style="width:100%;background:#10182a;border:1px solid var(--hair);border-radius:8px;color:var(--text);font-size:10.5px"></textarea>'
    + '<div class="btn-row" style="margin-top:8px"><button class="btn primary" data-a="plug2-src-do">校验并添加</button></div></div>'
    /* ③ 远程清单 */
    + '<div style="background:var(--raised);border:1px solid var(--hair);border-radius:12px;padding:12px 13px">'
    + '<b style="font-size:12.5px">🌐 远程清单</b>'
    + '<div class="sub" style="font-size:10.5px;margin:4px 0 8px;line-height:1.7">填第三方插件市场地址(http(s) 静态 JSON),拉取校验后上架。</div>'
    + (PLUG2.srcForm
      ? '<input class="mono" data-pk="r-id" placeholder="源 id(如 my-market)" style="width:100%;background:#10182a;border:1px solid var(--hair);border-radius:8px;color:var(--text);padding:7px 11px;font-size:11.5px;margin-bottom:6px">'
        + '<input class="mono" data-pk="r-name" placeholder="名称(如 我的市场)" style="width:100%;background:#10182a;border:1px solid var(--hair);border-radius:8px;color:var(--text);padding:7px 11px;font-size:11.5px;margin-bottom:6px">'
        + '<input class="mono" data-pk="r-url" placeholder="https://example.com/plugins.json" style="width:100%;background:#10182a;border:1px solid var(--hair);border-radius:8px;color:var(--text);padding:8px 11px;font-size:11.5px">'
        + '<div class="btn-row" style="margin-top:8px"><button class="btn primary" data-a="plug2-src-do">校验并添加</button><button class="btn ghost" data-a="plug2-src-toggle">取消</button></div>'
      : '<button class="btn ghost" data-a="plug2-src-toggle">＋ 添加远程源</button>')
    + '</div></div>';
  /* 我的插件/源列表 */
  if (mine.length) {
    html += '<table class="mtable" style="margin-top:14px"><thead><tr><th>我的插件 / 源</th><th style="width:12%">来源</th><th style="width:12%">状态</th><th style="width:18%">操作</th></tr></thead><tbody>'
      + mine.map(function (m, i) {
        return '<tr><td><b>' + esc(m.name) + '</b><br><span class="sub" style="font-size:10px">' + esc(m.detail || m.url || "") + '</span></td>'
          + '<td><span class="tag">' + esc(m.src || "远程") + '</span></td>'
          + '<td><span class="tag on" style="border-color:var(--c-official);color:var(--c-official)">已上架</span></td>'
          + '<td><button class="btn ghost" data-a="plug2-src-use" data-id="' + esc(m.id) + '">安装</button> <button class="btn ghost" data-a="plug2-local-del" data-i="' + i + '">移除</button></td></tr>';
      }).join("")
      + '</tbody></table>';
  } else {
    html += '<div class="sub" style="margin-top:14px">还没有自己设计的插件;用「本地添加」或「粘贴 manifest」做一个吧。</div>';
  }
  /* manifest 示例(全功能开放) */
  html += '<section class="card" style="margin-top:14px;background:rgba(80,200,130,.04);border-color:rgba(80,200,130,.25)"><h2 style="font-size:12.5px">📄 manifest 示例(全功能开放:挂载点 / 输入输出 / 配置项 / 模型故障转移 / 文档)</h2>'
    + '<pre class="mono" style="font-size:10.5px;line-height:1.7;color:#9FE3BC;white-space:pre-wrap;margin-top:6px">' + esc(JSON.stringify({
      id: "my-describer", name: "我的识图增强", version: "1.0.0", author: "你",
      mount: "media_parse",
      input: { required: ["media_url"], properties: { media_url: "string", prompt: "string" } },
      output: { properties: { text: "string" } },
      models: [
        { id: "gpt-5.6", api: "https://2xa.cc.cd/v1", note: "主模型:多模态直接识别" },
        { id: "deepseek-chat", api: "https://api.deepseek.com/v1", note: "备用:纯文本,经文字链路识别" }
      ],
      config: [{ k: "lang", label: "输出语言", type: "select", options: ["中文"] }],
      docs: "使用说明…"
    }, null, 2)) + '</pre></section>';
  return html;
}

/* 世界内嵌生态入口 strip:主通道卡下方一行,B 段按支持表渲染 */
function ecoStripHtml(agent) {
  if (!ECO_SUPPORTED_IDS[agent]) return "";
  return '<div style="margin-top:12px;padding:10px 12px;background:rgba(80,200,130,.05);border:1px solid rgba(80,200,130,.25);border-radius:10px;display:flex;align-items:center;justify-content:space-between;gap:10px">'
    + '<span style="font-size:12px;color:#A9E6C3">🌱 工具生态:该平台支持 MCP 服务器管理(生态中心)</span>'
    + '<button class="btn ghost" data-a="eco-jump" data-g="' + esc(agent) + '">管理 MCP →</button></div>';
}

function historyHtml() {  if (state.agent === "hermes") {
    /* Hermes 会话第一版不做(方案 §六);state.db 为 hermes 私有格式,后续批次再评估 */
    return '<section class="card" style="min-height:100%;display:flex;flex-direction:column;align-items:center;justify-content:center;text-align:center;gap:10px">'
      + '<div style="font-size:30px">🕘</div>'
      + '<h2 style="font-size:15px">Hermes 历史会话</h2>'
      + '<div class="sub" style="max-width:380px">Hermes 的会话历史功能第一版暂不提供;可在终端 hermes 内查看。</div>'
      + '</section>';
  }
  if (state.agent === "claude") {
    var cs = state.claudeSessions;
    var clist;
    if (cs === null) clist = '<div class="sub">加载中…</div>';
    else if (!cs.length) clist = '<div class="sub">还没有 Claude 会话。</div>';
    else {
      clist = cs.map(function (it) {
        return '<div class="hist-row"><b>' + esc(it.title || "(无标题)") + '</b>'
          + '<span class="meta">' + esc(fmtTime(it.updatedAt)) + (it.cwd ? " · " + esc(it.cwd) : "") + '</span></div>';
      }).join("");
      /* 首屏 50 条 + 加载更多(50/页追加);还有下一页才出按钮 */
      if (cs.length < state.claudeSessionsTotal) {
        clist += '<button class="btn ghost" data-a="csess-more" style="width:100%;margin-top:8px"' + (state.claudeSessionsLoading ? " disabled" : "") + '>'
          + (state.claudeSessionsLoading ? "加载中…" : "加载更多") + '</button>';
      }
    }
    return '<section class="card" style="min-height:100%"><h2>历史会话 · Claude</h2>'
      + '<div class="sub">Claude Code 的对话记录(~/.claude 统一保存),只读展示;与 Codex 的会话分开管理。'
      + (cs !== null ? ' 共 <b>' + state.claudeSessionsTotal + '</b> 条。' : "") + '</div>'
      + '<div class="btn-row"><button class="btn ghost" data-a="csess-refresh"' + (state.claudeSessionsLoading ? " disabled" : "") + '>刷新</button></div>'
      + '<div style="margin-top:10px">' + clist + '</div></section>';
  }
  var s = state.sessions;
  var listHtml;
  if (state.sessionsRepairing) listHtml = '<div class="sub">正在对账会话…(先整库备份,再核对会话文件)</div>';
  else if (s === null) listHtml = '<div class="sub">加载中…</div>';
  else if (!s.length) listHtml = '<div class="sub">没有会话记录。</div>';
  else {
    listHtml = s.map(function (it) {
      var tagColor = it.providerTag === "unknown" ? "" : 'style="border-color:var(--c-gw);color:var(--c-gw)"';
      return '<div class="hist-row"><b>' + esc(it.title || "(无标题)") + '</b>'
        + '<span class="meta">' + esc(it.providerTag) + ' · ' + esc(fmtTime(it.updatedAt)) + (it.cwd ? " · " + esc(it.cwd) : "") + '</span>'
        + '<span class="tag" ' + tagColor + '>' + esc(it.providerTag) + '</span>'
        + '<button class="btn sm ghost" data-a="sess-continue" data-i="' + esc(it.id) + '">继续</button></div>';
    }).join("");
  }
  var autoOn = !!(state.sessionsSettings && state.sessionsSettings.autoRepairBeforeHost);
  return '<section class="card" style="min-height:100%"><h2>历史会话</h2>'
    + '<div class="sub">Codex 对话记录(~/.codex 统一保存);修复前自动备份。共 <b>' + state.sessionsTotal + '</b> 条。</div>'
    + '<div class="btn-row">'
    + '<button class="btn ghost" data-a="sess-repair"' + (state.sessionsRepairing ? " disabled" : "") + '>' + (state.sessionsRepairing ? "修复中…" : "立刻修复历史会话") + '</button>'
    + '<label style="display:flex;align-items:center;gap:6px;font-size:12px;color:var(--muted);cursor:pointer;padding:3px 0"><input type="checkbox" data-a="sess-autofix"' + (autoOn ? " checked" : "") + '>启动前自动修复</label>'
    + '</div>'
    + '<div style="margin-top:10px">' + listHtml + '</div></section>';
}
async function loadSessions() {
  try {
    var d = await api.sessions(1, 50, "");
    state.sessions = d.items || [];
    state.sessionsTotal = d.total || 0;
  } catch (e) {
    state.sessions = [];
    showToast("获取会话失败:" + e.message, "error");
  }
  render();
}
async function loadSessionsSettings() {
  try { state.sessionsSettings = await api.sessionsSettings(); } catch (e) { state.sessionsSettings = null; }
  render();
}
/* Claude 会话列表(只读):reset=true 重拉首页;false 追加下一页(50/页) */
async function loadClaudeSessions(reset) {
  state.claudeSessionsLoading = true;
  if (reset) { state.claudeSessions = null; state.claudeSessionsPage = 0; }
  render();
  var page = (state.claudeSessionsPage || 0) + 1;
  try {
    var d = await api.claudeSessions(page, 50);
    var items = d.items || [];
    state.claudeSessions = page === 1 ? items : (state.claudeSessions || []).concat(items);
    state.claudeSessionsTotal = d.total || 0;
    state.claudeSessionsPage = page;
  } catch (e) {
    if (reset) state.claudeSessions = [];
    showToast("获取 Claude 会话失败:" + e.message, "error");
  }
  state.claudeSessionsLoading = false;
  render();
}
async function doSessionsRepair() {
  state.sessionsRepairing = true; render();
  try {
    var d = await api.sessionsRepair();
    showToast("修复完成:对账 " + d.scanned + " 条,修正 " + d.fixed + " 条(已先备份)", "ok");
  } catch (e) { showToast("修复失败:" + e.message, "error"); }
  state.sessionsRepairing = false;
  await loadSessions();
}

/* ── 编辑供应商弹窗 ── */
function renderModelRows() {
  var tb = document.getElementById("modelRows"); if (!tb || !state.edit) return;
  var rows = (state.edit.models || []).map(function (m, i) {
    return '<tr><td><input data-mf="name" data-mi="' + i + '" value="' + esc(m.name || "") + '"></td>'
      + '<td><input data-mf="cw" data-mi="' + i + '" style="width:80px" value="' + esc(m.contextWindow || "") + '"></td>'
      + '<td><button class="btn ghost danger sm" data-a="mrow-del" data-i="' + i + '">✕</button></td></tr>';
  }).join("");
  tb.innerHTML = rows || '<tr><td colspan="3" style="color:var(--muted)">还没有模型;点「拉取模型」自动填写。</td></tr>';
}
/* ── 预设供应商选择层(presets.js 目录:2xapi 置顶+27 家厂商;选中回填编辑弹窗) ── */
var presetQuery = "";
function openPresetPicker() {
  if (!window.PRESETS || !window.PRESETS.length) { openEdit(null); return; }
  presetQuery = "";
  document.getElementById("presetSearch").value = "";
  document.getElementById("presetMask").style.display = "";
  renderPresetGrid();
}
function closePresetPicker() {
  document.getElementById("presetMask").style.display = "none";
}
function renderPresetGrid() {
  var q = presetQuery.trim().toLowerCase();
  var list = window.PRESETS.filter(function (p) {
    return !q || p.name.toLowerCase().indexOf(q) >= 0 || (p.baseUrl || "").toLowerCase().indexOf(q) >= 0;
  });
  document.getElementById("presetGrid").innerHTML = list.map(function (p) {
    var top = p.top ? " top" : "";
    return '<button class="preset-item' + top + '" data-a="preset-pick" data-id="' + esc(p.id) + '">'
      + '<img src="' + esc(p.logo) + '" alt="">'
      + '<span class="pn">' + esc(p.name) + '</span>'
      + '<span class="pe">' + esc((p.baseUrl || "自填端点").replace(/^https?:\/\//, "").split("/")[0] || "自填端点") + '</span>'
      + '</button>';
  }).join("") || '<div class="sub" style="padding:20px;text-align:center">没有匹配的预设</div>';
}
function applyPreset(pid) {
  var p = (window.PRESETS || []).find(function (x) { return x.id === pid; });
  closePresetPicker();
  openEdit(null); /* 先建空草稿,再回填 */
  if (!p) return;
  state.edit.name = p.name;
  state.edit.baseUrl = p.baseUrl || "";
  state.edit.iconColor = p.iconColor || null;
  state.edit.icon = p.id;
  document.getElementById("eName").value = p.name;
  document.getElementById("eUrl").value = p.baseUrl || "";
  /* 协议保持「自动」:网关自动转译+拉取模型时探测回写,不预填 */
  var w = document.getElementById("eWire");
  if (w) w.value = "auto";
  showToast(p.top
    ? "已填入「" + p.name + "」预设:API Key 填站内的 sk- 开头密钥;若「拉取模型」被站点限制,点「＋ 手动加一行」填模型名即可"
    : "已填入「" + p.name + "」预设:补上 API Key,点「拉取模型」选默认模型即可", "ok");
}

function openEdit(id) {
  var p = id ? lineOf(id) : null;
  var isC = state.agent === "claude";
  var isH = state.agent === "hermes";
  var defWire = isC ? "anthropic" : (isH ? "chat_completions" : "responses");
  state.edit = p
    ? { id: p.id, isNew: false, name: p.name, baseUrl: p.baseUrl || "", apiKey: "", model: p.model || "", wireApi: p.wireApi || defWire, icon: p.icon || null, iconColor: p.iconColor || null, ua: p.userAgent || null, models: (p.models || []).map(normModel) }
    : { id: null, isNew: true, name: "", baseUrl: "", apiKey: "", model: "", wireApi: defWire, icon: null, iconColor: null, ua: null, models: [] };
  state.fieldErrors = {};
  document.getElementById("editTitle").textContent = (p ? "编辑供应商 · " + p.name : "新建供应商") + " · " + (isC ? "Claude" : (isH ? "Hermes" : "Codex"));
  document.getElementById("eName").value = state.edit.name;
  document.getElementById("eUrl").value = state.edit.baseUrl;
  document.getElementById("eKey").value = "";
  document.getElementById("eKey").placeholder = state.edit.isNew ? (isC ? "sk-ant-..." : "sk-...") : (p.apiKeyMasked ? "•••• 未改则留空" : (isC ? "sk-ant-..." : "sk-..."));
  document.getElementById("eModel").value = state.edit.model;
  var wSel = document.getElementById("eWire");
  /* 新建一律显示「自动」(保存时落世界默认值,拉取模型探测后回写);编辑已有供应商显示当前实际协议 */
  if (wSel) wSel.value = state.edit.isNew ? "auto" : ((["responses","chat_completions","anthropic"].indexOf(state.edit.wireApi) >= 0) ? state.edit.wireApi : "auto");
  var uaSel = document.getElementById("eUa");
  if (uaSel) {
    uaSel.value = state.edit.ua || "";
    if (state.edit.ua) {
      var uaKnown = Array.prototype.some.call(uaSel.options, function (o) { return o.value === state.edit.ua; });
      if (!uaKnown) { var uaOpt = document.createElement("option"); uaOpt.value = state.edit.ua; uaOpt.text = "自定义"; uaSel.appendChild(uaOpt); }
    }
  }
  renderModelRows();
  document.getElementById("editMask").style.display = "";
}
function collectEdit() {
  return {
    name: $("#eName").value.trim(),
    baseUrl: $("#eUrl").value.trim(),
    apiKey: $("#eKey").value,
    model: $("#eModel").value.trim(),
  };
}
function readModelRows() {
  var out = [];
  document.querySelectorAll('#modelRows input[data-mf="name"]').forEach(function (inp) {
    var i = Number(inp.dataset.mi);
    var cwEl = document.querySelector('#modelRows input[data-mf="cw"][data-mi="' + i + '"]');
    var cw = cwEl ? cwEl.value.trim() : "";
    var nm = inp.value.trim();
    if (nm) out.push({ name: nm, contextWindow: cw ? Number(cw) : null });
  });
  return out;
}
function closeEdit() { document.getElementById("editMask").style.display = "none"; state.edit = null; render(); }
async function doSaveEdit() {
  var d = collectEdit();
  var errs = {};
  if (!d.name) errs.name = "必填";
  if (!d.baseUrl) errs.baseUrl = "必填";
  if (state.edit.isNew && !d.apiKey) errs.apiKey = "新建必填";
  var models = readModelRows();
  if (errs.name || errs.baseUrl || errs.apiKey) {
    state.fieldErrors = errs;
    showToast("还有必填项未完成", "error");
    return;
  }
  var model = d.model || (models.length ? models[0].name : "");
  var body = {
    name: d.name, accessMode: "pure_api", model: model,
    baseUrl: d.baseUrl, apiKey: d.apiKey || "",
    wireApi: (function () { var w = document.getElementById("eWire"); var v = w ? w.value : "auto"; return v === "auto" ? state.edit.wireApi : v; })(), models: models,
    proxyUrl: "", timeoutSecs: null, notes: "", reasoning_levels: [],
    userAgent: (document.getElementById("eUa") || { value: "" }).value || null,
    icon: state.edit.icon || null,
    iconColor: state.edit.iconColor || null,
    agent: state.agent,
  };
  state.busy = "save"; render();
  try {
    var saved = state.edit.isNew ? await api.createProvider(body) : await api.updateProvider(state.edit.id, body);
    state.fieldErrors = {};
    await refreshProviders();
    state.selId = (saved && (saved.id || (saved.provider && saved.provider.id))) || state.selId;
    closeEdit();
    showToast("供应商已保存(仅存于本软件,未写任何配置)", "ok");
  } catch (e) {
    state.fieldErrors = {};
    showToast(e.message, "error");
  }
  state.busy = null; render();
}
async function doFetchModels() {
  var d = collectEdit();
  var fmBody = (!state.edit.isNew && state.edit.id) ? { id: state.edit.id } : { baseUrl: d.baseUrl, apiKey: d.apiKey };
  if (!state.edit.isNew && d.apiKey) fmBody = { id: state.edit.id, apiKey: d.apiKey };
  if (fmBody.baseUrl !== undefined && (!fmBody.baseUrl || !fmBody.apiKey)) {
    showToast("新建供应商请先填上游地址和 api key", "error"); return;
  }
  state.busy = "mfetch"; render();
  try {
    var r = await api.fetchModels(fmBody);
    state.edit.models = (r.models || []).map(normModel);
    state.edit.reasoning_levels = Array.isArray(r.reasoning_levels) ? r.reasoning_levels.slice() : [];
    if (!d.model && state.edit.models.length) $("#eModel").value = state.edit.models[0].name;
    renderModelRows();
    showToast("拉取到 " + state.edit.models.length + " 个模型" + (state.edit.models.length ? ",默认模型已填入" : ""), "ok");
  } catch (e) {
    showToast("拉取模型失败:" + e.message, "error");
  }
  state.busy = null; render();
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

/* ── 托管 / 还原 ── */
async function doHost(providerId, way) {
  if (!providerId) return;
  state.busy = "host"; render();
  try {
    if (state.agent === "hermes") {
      /* Hermes 叠加托管:固定 gateway 通路,走泛化路由 */
      var r = await api.agentHost("hermes", providerId, "gateway");
      await refreshHermesState();
      state.selId = providerId;
      showToast(r && r.pointerSwitched === false
        ? "条目已写入;Hermes 当前默认模型指向你的第三方供应商,未自动切换(可在 hermes 内自选)"
        : "Hermes 已托管走中转(叠加条目已写入,已自动备份,可随时还原)", "ok");
    } else if (GW_AGENTS[state.agent]) {
      /* 通用平台世界:固定 gateway 通路,走泛化路由 */
      var g = await api.agentHost(state.agent, providerId, "gateway");
      await refreshGwState(state.agent);
      state.selId = providerId;
      var restartNote = GW_META[state.agent] && GW_META[state.agent].restart ? "(重启客户端后生效)" : "";
      showToast(g && g.suggested
        ? "条目已写入;当前默认模型未自动切换(可在客户端内自选)" + restartNote
        : "已托管走中转(可随时还原)" + restartNote, "ok");
    } else {
      var w = (way === "direct" || way === "gateway") ? way : codexWayNow();
      var r2 = await api.desktopHost(providerId, w);
      state.codexWay = w;
      await refreshAll();
      state.selId = providerId;
      showToast(r2 && r2.switched
        ? "已切换供应商(即时生效)"
        : (w === "direct"
          ? "桌面版已直连托管(Key 已写入本地配置,已自动备份,可随时还原)"
          : "桌面版已托管走中转(已自动备份,可随时还原)"), "ok");
    }
  } catch (e) {
    showToast(e.message, "error");
    if (state.agent === "hermes") await refreshHermesState(); else await refreshDesktop();
  }
  state.busy = null; render();
}
async function doUnhost() {
  state.busy = "unhost"; render();
  try {
    if (state.agent === "hermes") {
      var r = await api.agentUnhost("hermes");
      await refreshHermesState();
      showToast(r && r.restored ? "已还原(条目已移除,指针已恢复;可从备份目录恢复)" : "当前未托管,无需还原", "ok");
    } else if (GW_AGENTS[state.agent]) {
      var g2 = await api.agentUnhost(state.agent);
      await refreshGwState(state.agent);
      showToast(g2 && g2.restored ? "已还原" + (GW_META[state.agent] && GW_META[state.agent].restart ? "(重启客户端后生效)" : "") : "当前未托管,无需还原", "ok");
    } else {
      var r2 = await api.desktopUnhost();
      await refreshAll();
      showToast(r2 && r2.restored ? "已还原(可从备份目录恢复)" : "当前未托管,无需还原", "ok");
    }
  } catch (e) {
    showToast(e.message, "error");
    if (state.agent === "hermes") await refreshHermesState(); else await refreshDesktop();
  }
  state.busy = null; render();
}

/* ── Claude 注入式托管(后端返回注入信息,前端展示/复制;停用=前端本地态)── */
async function doClaudeStart(providerId) {
  var pid = providerId || state.selId;
  if (!pid) { showToast("请先选择或新建一个供应商", "error"); return; }
  state.busy = "claude-start"; render();
  try {
    var r = await api.claudeStart(claudeWay(), pid);
    state.claude = {
      started: true,
      way: r.way || claudeWay(),
      providerId: r.providerId || pid,
      providerName: r.providerName || (lineOf(r.providerId || pid) || {}).name || "",
      env: r.env || {},
      command: r.command || "",
      model: r.model || "",
    };
    state.selId = state.claude.providerId;
    showToast("已注入环境变量,在终端运行 claude 即可", "ok");
  } catch (e) {
    state.claude = null;
    showToast("注入失败:" + e.message, "error");
  }
  state.busy = null; render();
}
function doClaudeStop() {
  /* 后端无 claude-stop 接口(注入式无常驻进程):停用 = 清除前端注入态 */
  state.claude = null;
  state.busy = null;
  render();
  showToast("已停用注入", "ok");
}
function copyText(t) {
  if (navigator.clipboard && navigator.clipboard.writeText) {
    return navigator.clipboard.writeText(t).catch(function () { return fallbackCopy(t); });
  }
  return Promise.resolve(fallbackCopy(t));
}
function fallbackCopy(t) {
  var ta = document.createElement("textarea");
  ta.value = t; ta.style.position = "fixed"; ta.style.opacity = "0";
  document.body.appendChild(ta); ta.select();
  try { document.execCommand("copy"); } catch (e) {}
  document.body.removeChild(ta);
  return true;
}

/* ── 加速(二态:off ↔ official;线路维护在 设置 → IP 管理)── */
async function doAccel(m) {
  var mode = m === "on" ? "official" : "off";
  var acc = state.accel || {};
  if (acc.mode === mode) { render(); return; }
  state.busy = "accel"; render();
  try {
    await api.accelSetMode(mode);
    await refreshAccel();
    showToast(mode === "off" ? "加速已关闭,网关直发上游" : "加速已开启(线路自动择优)", "ok");
  } catch (e) { showToast(e.message, "error"); }
  state.busy = null; render();
}

/* ── 登录(2xapi 账号:邮箱/密码 + 记住我;站点开启验证码时弹滑块)── */
var captchaCfg = { enabled: false, appId: "", loaded: false };
function loadTcaptchaJs(cb) {
  if (window.TencentCaptcha || captchaCfg.loaded) return cb();
  captchaCfg.loaded = true;
  var s = document.createElement("script");
  s.src = "https://turing.captcha.qcloud.com/TCaptcha.js";
  s.onload = function () { cb(); };
  s.onerror = function () { captchaCfg.loaded = false; state.loginError = "验证码组件加载失败,请检查网络"; renderLoginForm(); };
  document.head.appendChild(s);
}
function renderLoginForm() {
  var f = document.getElementById("loginForm"); if (!f) return;
  f.innerHTML =
    '<div class="f" style="margin:8px 0"><label>邮箱</label><input data-l="email" value="' + esc(state.loginEmail) + '"></div>'
    + '<div class="f" style="margin:8px 0"><label>密码</label><input type="password" data-l="password" value="' + esc(state.loginPassword) + '"></div>'
    + (captchaCfg.enabled ? '<div class="sub" style="margin:0 0 6px;color:var(--c-direct)">该站点开启了登录验证,点「登录」后请完成滑块验证</div>' : "")
    + '<label style="display:flex;align-items:center;gap:8px;font-size:12.5px;color:var(--muted);cursor:pointer;margin:2px 0 8px"><input type="checkbox" data-l="remember" checked>记住我(保持登录,滑块只需这一次)</label>'
    + (state.loginError ? '<div style="color:var(--c-err);font-size:12px;margin:0 0 4px">' + esc(state.loginError) + '</div>' : "");
}
function openLogin() {
  state.loginError = "";
  document.getElementById("loginMask").style.display = "";
  renderLoginForm();
  api.remembered().then(function (r) {
    if (r && r.remembered) {
      state.loginEmail = r.email || "";
      state.loginPassword = r.password || "";
      state.remembered = true;
      renderLoginForm();
    }
  }).catch(function () {});
  api.captchaSettings().then(function (c) {
    captchaCfg.enabled = !!(c && c.enabled);
    captchaCfg.appId = (c && String(c.appId || "")) || "";
    if (captchaCfg.enabled) { loadTcaptchaJs(function () {}); renderLoginForm(); }
  }).catch(function () {});
}
async function doLogin() {
  var email = $('#loginForm [data-l="email"]').value.trim();
  var password = $('#loginForm [data-l="password"]').value;
  if (!email || !password) { state.loginError = "邮箱和密码都要填"; renderLoginForm(); return; }
  var submit = async function (ticket, randstr) {
    try {
      await api.login(email, password, ticket, randstr);
      var remember = ($('#loginForm [data-l="remember"]') || {}).checked !== false;
      try { remember ? await api.remember(email, password) : await api.forget(); } catch (e) { /* 记住失败不影响登录 */ }
      document.getElementById("loginMask").style.display = "none";
      state.loginError = "";
      await refreshSession();
      showToast("登录成功" + (remember ? "(已记住,下次自动保持)" : ""), "ok");
      if (!state.providers.length) openImport(); // 行业惯例:登录成功且无供应商 → 自动弹导入向导
      render();
    } catch (e) {
      state.loginError = e.message; renderLoginForm();
    }
  };
  if (captchaCfg.enabled && captchaCfg.appId) {
    loadTcaptchaJs(function () {
      if (!window.TencentCaptcha) { state.loginError = "验证码组件未就绪,请重试"; renderLoginForm(); return; }
      var cap = new window.TencentCaptcha(captchaCfg.appId, function (res) {
        if (res && res.ret === 0) submit(res.ticket, res.randstr);
      });
      cap.show();
    });
  } else {
    submit("", "");
  }
}
async function doLogout() {
  try { await api.logout(); } catch (e) {}
  try { await api.forget(); } catch (e) {}
  state.session = null; state.balance = null; state.menuOpen = false;
  render();
  showToast("已登出", "ok");
}

/* ── 一键导入 Key 向导 ── */
async function openImport() {
  state.menuOpen = false; renderTopAuth();
  var keys = [], baseUrl = "";
  try {
    var d = await api.apiKeys();
    var raw = d || [];
    keys = Array.isArray(raw) ? raw : ((raw && raw.keys) || []); // 后端契约 {keys,baseUrl};部分实现直接给数组
    baseUrl = (!Array.isArray(raw) && raw && raw.baseUrl) || "";
  } catch (e) {
    showToast("获取 Key 列表失败:" + e.message + (String(e.message).indexOf("登录") >= 0 ? ",请先登录" : ""), "error");
    return;
  }
  state.importKeys = { keys: keys, baseUrl: baseUrl };
  state.importBusy = false;
  document.getElementById("impMask").style.display = "";
  renderImport();
}
function renderImport() {
  var body = document.getElementById("impBody"); if (!body) return;
  var d = state.importKeys;
  if (!d) { body.innerHTML = '<div class="sub">正在获取你的 Key 列表…</div>'; return; }
  if (!d.keys.length) {
    body.innerHTML = '<div class="sub">账号里还没有 Key,去 2xapi 网站创建后再来导入。</div>';
    return;
  }
  body.innerHTML = d.keys.map(function (k, i) {
    var keyStr = String(k.key || "");
    var masked = keyStr.length > 12 ? keyStr.slice(0, 6) + "…" + keyStr.slice(-4) : keyStr;
    var active = k.status === "active" || k.status === "enabled" || !k.status;
    var quota = (typeof k.quota === "number" && k.quota > 0)
      ? " · 额度 $" + k.quota.toFixed(2) + (k.quota_used ? "(已用 $" + Number(k.quota_used).toFixed(2) + ")" : "")
      : " · 不限量";
    return '<div class="hist-row" style="cursor:pointer"><input type="checkbox" class="imp-cb" data-i="' + i + '" checked style="width:auto;flex:none">'
      + '<div style="flex:1;min-width:0"><b style="font:600 11.5px var(--mono)">' + esc(k.name || ("Key " + (i + 1))) + '</b>'
      + '<span style="display:block;font-size:11px;color:var(--muted)">' + esc(masked) + quota + (active ? "" : ' · <span style="color:var(--c-err)">' + esc(k.status) + "</span>") + '</span></div>'
      + '<span class="tag">将生成供应商</span></div>';
  }).join("")
    + (state.importBusy ? '<div class="sub" style="margin-top:6px">导入中…(逐 Key 拉模型、建供应商)</div>'
      : '<div class="sub" style="margin-top:6px">导入后自动拉取模型、填写默认模型,无需手动配置。</div>');
}
async function doImport() {
  var d = state.importKeys;
  if (!d || !d.keys.length) return;
  var selIdx = [];
  document.querySelectorAll('#impBody .imp-cb:checked').forEach(function (cb) { selIdx.push(Number(cb.dataset.i)); });
  if (!selIdx.length) { showToast("请先勾选要导入的 Key", "error"); return; }
  state.importBusy = true; renderImport();
  var ok = 0, fail = [];
  for (var n = 0; n < selIdx.length; n++) {
    var k = d.keys[selIdx[n]];
    if (!k) continue;
    try {
      var fm = await api.fetchModels({ baseUrl: d.baseUrl, apiKey: k.key });
      var models = (fm.models || []).map(normModel);
      if (!models.length) { fail.push((k.name || "Key") + ":拉不到模型"); continue; }
      var name = (k.name && String(k.name).trim()) || ("2xapi-" + String(k.key || "").slice(-6));
      if (state.providers.some(function (p) { return p.name === name; })) name += " 2";
      await api.createProvider({
        name: name, accessMode: "pure_api", baseUrl: d.baseUrl, apiKey: k.key, wireApi: "responses",
        model: models[0].name, models: models,
        reasoning_levels: Array.isArray(fm.reasoning_levels) ? fm.reasoning_levels : [],
        agent: state.agent,
      });
      ok++;
    } catch (e) { fail.push((k.name || "Key") + ":" + e.message); }
  }
  await refreshProviders();
  document.getElementById("impMask").style.display = "none";
  state.importBusy = false;
  showToast(ok ? ("已导入 " + ok + " 个供应商" + (fail.length ? "(" + fail.length + " 失败)" : "")) : ("导入失败:" + fail[0]), ok ? "ok" : "err");
  render();
}

/* ── ⚙ 设置弹窗:五分区 ── */
var SET_TABS = [["ip", "IP 管理"], ["usage", "用量"], ["account", "账号"], ["general", "通用"], ["advanced", "高级"], ["about", "关于"]];
function openSettings() {
  state.setTab = "ip";
  state.menuOpen = false;
  renderSettings();
  document.getElementById("setMask").style.display = "";
}
function renderSettings() {
  var tabs = document.getElementById("setTabs"); if (!tabs) return;
  tabs.innerHTML = SET_TABS.map(function (x) {
    return '<button class="set-tab ' + (state.setTab === x[0] ? "on" : "") + '" data-a="set-tab" data-s="' + x[0] + '">' + x[1] + '</button>';
  }).join("");
  var body = document.getElementById("setBody"); if (!body) return;
  body.innerHTML =
    state.setTab === "ip" ? setIpHtml()
    : state.setTab === "usage" ? setUsageHtml()
    : state.setTab === "account" ? setAccountHtml()
    : state.setTab === "general" ? setGeneralHtml()
    : state.setTab === "advanced" ? setAdvancedHtml()
    : setAboutHtml();
}
function setRow(label, ctrl, hint) {
  return '<div style="display:flex;align-items:center;gap:12px;padding:9px 0;border-bottom:1px solid var(--hair)">'
    + '<div style="flex:1;min-width:0"><div style="font-size:12.5px">' + label + '</div>' + (hint ? '<div class="sub">' + hint + '</div>' : '') + '</div>'
    + '<div style="flex:none">' + ctrl + '</div></div>';
}
function setIpHtml() {
  var acc = state.accel || {};
  var offLines = (acc.lines || []).filter(function (l) { return l.enabled !== false; });
  var myEp = acc.customNode || "";
  var at = state.nodeTest;
  var testHtml;
  if (at && at.busy) testHtml = '<div class="sub" style="margin:6px 0 0">连通测试中…</div>';
  else if (at && at.ok) testHtml = '<div class="sub" style="margin:6px 0 0;color:var(--c-official)">✓ 连通 · 延迟 ' + at.latencyMs + 'ms</div>';
  else if (at && !at.ok) testHtml = '<div class="sub" style="margin:6px 0 0;color:var(--c-err)">✗ ' + esc(at.msg) + '</div>';
  else testHtml = "";
  var nodeVal = (state.nodeDraft != null ? state.nodeDraft : myEp);
  return '<h3 style="margin:2px 0 4px;font-size:13.5px">IP 管理 · 加速线路</h3>'
    + '<div class="sub" style="margin-bottom:6px">官方内置自动下发(本期只读展示);自己的代理随时加,仅本机保存。加速开启时从「已启用」线路混合择优。</div>'
    + '<div class="eyebrow" style="margin:8px 0 6px">官方内置(自动下发 · 只读)</div>'
    + (offLines.length ? offLines.map(function (l) {
      return '<div class="hist-row"><b style="min-width:92px">' + esc(l.name) + '</b>'
        + '<span class="meta" style="font-family:var(--mono);font-size:11px">' + esc(l.endpoint || l.scope || "自动下发") + '</span>'
        + '<span class="tag" style="border-color:var(--c-gw);color:var(--c-gw)">官方</span>'
        + '<span class="tag">' + (l.latency ? l.latency + "ms" : "—") + '</span></div>';
    }).join("") : '<div class="sub" style="padding:4px 0">暂无官方线路下发。</div>')
    + '<div class="eyebrow" style="margin:14px 0 6px">我的代理(自己添加 · 仅本机保存)</div>'
    + (myEp
      ? '<div class="hist-row"><b style="min-width:92px">我的代理</b>'
        + '<span class="meta" style="font-family:var(--mono);font-size:11px">' + esc(myEp) + '</span>'
        + '<span class="tag">我的</span>'
        + '<button class="btn sm ghost danger" data-a="ipm-del">删除</button></div>'
      : '<div class="sub" style="padding:4px 0">还没有自己的代理;在下面添加一条。</div>')
    + '<div class="mg-tools" style="margin-top:8px"><input class="mono" id="ipmNew" data-a="ipm-new-input" style="flex:1;min-width:0;padding:6px 9px;background:var(--raised);border:1px solid var(--hair);border-radius:7px;color:var(--text);font:11.5px var(--mono)" placeholder="socks5://127.0.0.1:7890 或 http://用户:密码@你的VPS:443" value="' + esc(nodeVal) + '">'
    + '<button class="btn primary" data-a="ipm-add"' + (state.busy === "ipm" ? " disabled" : "") + '>＋ 添加</button>'
    + '<button class="btn ghost" data-a="ipm-test"' + (state.busy === "ipm" ? " disabled" : "") + '>测试连通</button></div>'
    + testHtml
    + '<div class="eyebrow" style="margin:14px 0 6px">官方加速凭证(每账号配额)</div>'
    + setRow('官方加速凭证', '<button class="btn sm ghost" data-a="ipm-refresh"' + (state.busy === "ipm" ? " disabled" : "") + '>' + (state.busy === "ipm" ? "刷新中…" : "刷新凭证") + '</button>',
      usageLine(acc.usage) + ';配额用满会自动切直连,恢复后刷新即可重新加速。');
}
function usageLine(usage) {
  var u = (usage && usage.ok) ? usage : null;
  if (!u) return "当前未换取凭证";
  return "用量 " + fmtGb(u.quotaUsedBytes) + " G / " + fmtQuotaTotalGb(u.quotaTotalBytes) + " G" + (u.degradedToDirect ? "(已降级直连)" : "");
}
function fmtGb(bytes) { return (Number(bytes || 0) / 1073741824).toFixed(2); }
function fmtQuotaTotalGb(bytes) { return String(Math.round(Number(bytes || 10737418240) / 1073741824)); }
async function doIpmAdd() {
  var el = document.getElementById("ipmNew");
  var endpoint = el ? el.value.trim() : "";
  if (!endpoint) { showToast("请先填写代理地址", "error"); return; }
  state.busy = "ipm"; renderSettings();
  try {
    await api.accelSetCustomNode(endpoint);
    state.nodeDraft = "";
    await refreshAccel();
    showToast("我的代理已保存(仅本机)", "ok");
  } catch (e) { showToast(e.message, "error"); }
  state.busy = null; renderSettings();
}
async function doIpmDel() {
  state.busy = "ipm"; renderSettings();
  try {
    await api.accelSetCustomNode("");
    state.nodeDraft = "";
    await refreshAccel();
    showToast("我的代理已删除(仅本机)", "ok");
  } catch (e) { showToast(e.message, "error"); }
  state.busy = null; renderSettings();
}
async function doIpmTest() {
  var el = document.getElementById("ipmNew");
  var endpoint = (el ? el.value.trim() : "") || (state.accel && state.accel.customNode) || "";
  if (!endpoint) { showToast("请先填写代理地址", "error"); return; }
  state.nodeTest = { busy: true }; renderSettings();
  try {
    var d = await api.accelTestNode(endpoint);
    state.nodeTest = { ok: true, latencyMs: d.latencyMs };
  } catch (e) { state.nodeTest = { ok: false, msg: e.message }; }
  renderSettings();
}
async function doIpmRefresh() {
  state.busy = "ipm"; renderSettings();
  try {
    var r = await api.accelRefreshCred();
    await refreshAccel();
    var u = (r && r.usage) || {};
    showToast(u && u.ok ? ("已刷新:用量 " + fmtGb(u.quotaUsedBytes) + " G / " + fmtQuotaTotalGb(u.quotaTotalBytes) + " G") : "已刷新", "ok");
  } catch (e) { showToast(e.message, "error"); }
  state.busy = null; renderSettings();
}
function setUsageHtml() {
  var u = state.usage;
  var rows;
  if (u && u.err) rows = '<div class="sub" style="color:var(--c-err);padding:4px 0">✗ ' + esc(u.err) + '</div>';
  else if (u && u.busy) rows = '<div class="sub" style="padding:4px 0">加载中…</div>';
  else if (u && u.providers && u.providers.length) {
    rows = u.providers.map(function (p) {
      var okRate = (p.okRate != null) ? Math.round(p.okRate * 100) + "%" : "—";
      return '<div class="hist-row" style="align-items:flex-start">'
        + '<b style="min-width:96px">' + esc(p.providerName || p.providerId) + '</b>'
        + '<span class="meta">' + p.count + ' 次请求</span>'
        + '<span class="tag">P50 ' + (p.p50Ms != null ? p.p50Ms + "ms" : "—") + '</span>'
        + '<span class="tag">P90 ' + (p.p90Ms != null ? p.p90Ms + "ms" : "—") + '</span>'
        + '<span class="tag">成功率 ' + okRate + '</span>'
        + (p.lastTs ? '<span class="tag">' + fmtTime(p.lastTs) + '</span>' : '')
        + (p.routes && p.routes.length ? '<div class="sub" style="width:100%;font-family:var(--mono);font-size:11px">' + esc(p.routes.join(" · ")) + '</div>' : '')
        + '</div>';
    }).join("");
  } else rows = '<div class="sub" style="padding:4px 0">暂无请求记录。使用网关转发对话后,这里会自动按供应商积累 P50/P90 延迟与成功率基准。</div>';
  return '<h3 style="margin:2px 0 4px;font-size:13.5px">用量仪表盘 · 网关统计</h3>'
    + '<div class="sub" style="margin-bottom:6px">每次经网关的请求自动落台账(不记录密钥);P50/P90 随使用自然累积,可作各供应商的延迟基准。</div>'
    + '<div style="margin:6px 0 10px"><button class="btn sm" data-a="usage-refresh"' + (u && u.busy ? " disabled" : "") + '>↻ 刷新</button></div>'
    + rows;
}
async function doUsageRefresh() {
  var u = state.usage;
  if (u && u.busy) return;
  state.usage = { busy: true };
  renderSettings();
  try {
    state.usage = { providers: await api.usageStats() };
  } catch (e) {
    state.usage = { err: (e && e.message) || "加载用量统计失败" };
  }
  renderSettings();
}
function setAccountHtml() {
  var email = sessionEmail();
  var bal = state.balance;
  var balText = bal == null ? "…" : (bal < 0 ? "-" : "") + "$" + Math.abs(bal).toFixed(2);
  var rem = state.remembered;
  if (!rem && state.session && loggedIn()) {
    api.remembered().then(function (r) { if (r && r.remembered) { state.remembered = true; if (state.setTab === "account") renderSettings(); } }).catch(function () {});
  }
  return '<h3 style="margin:2px 0 4px;font-size:13.5px">账号</h3>'
    + setRow('已登录:' + esc(email), '<button class="btn sm ghost" data-a="logout">登出</button>',
      '余额 ' + balText + (rem ? ' · 记住我 ✓(下次免滑块)' : ' · 未记住'))
    + setRow('一键导入 Key', '<span class="tag">入口在顶栏 ⇭</span>', '顶栏蓝色入口,登录后即见——这是最常用的动作,不藏在这里')
    + setRow('顶栏实时余额', '<button class="btn sm ghost" data-a="bal-toggle">' + (state.balShow ? "开" : "关") + '</button>', '余额不足 $1 时警示变色');
}
function setGeneralHtml() {
  /* 惰性加载自启状态(仿 setAccountHtml 的 remembered 模式;失败不阻塞渲染) */
  if (state.autostart === undefined) {
    api.autostart().then(function (r) {
      state.autostart = !!(r && r.enabled);
      if (state.setTab === "general") renderSettings();
    }).catch(function () { state.autostart = false; });
  }
  return '<h3 style="margin:2px 0 4px;font-size:13.5px">通用</h3>'
    + setRow('开机自动启动', '<button class="btn sm ghost" data-a="autostart-toggle">' + (state.autostart ? "开" : "关") + '</button>', '登录 macOS 时后台常驻,网关保持可用;登记于 launchd 的 com.2xapi.codexconsole')
    + setRow('关闭窗口时', '<span class="tag">隐藏到托盘</span>', '点托盘图标可再次打开;托盘菜单「退出」才真正关闭')
    + setRow('界面语言', '<span class="tag">跟随系统</span>', '')
    + '<div class="sub" style="margin-top:10px">托盘「网关开/关」可随时停用网关(内存态,重启恢复);自启开关为即时生效。</div>';
}
async function doAutostartToggle() {
  var next = !state.autostart;
  state.autostart = next;
  renderSettings();
  try {
    var r = await api.setAutostart(next);
    state.autostart = !!(r && r.enabled);
    showToast(state.autostart ? "已开启开机自启(登录时后台常驻)" : "已关闭开机自启", "ok");
  } catch (e) {
    state.autostart = !next;
    showToast(e.message || "设置失败", "error");
  }
  renderSettings();
}
function setAdvancedHtml() {
  return '<h3 style="margin:2px 0 4px;font-size:13.5px">高级</h3>'
    + setRow('Codex CLI 路径(自动检测)', '<button class="btn sm ghost" data-a="adv-recodex">重新检测</button>',
      '<span class="mono" style="font-size:10px">/Applications/ChatGPT.app/…/codex</span> · 环境变量 CODEX_CLI_PATH 可覆盖')
    + setRow('运行日志', '<button class="btn sm ghost" data-a="adv-inspect">查看</button>', '排障用;含网关请求摘要')
    + setRow('供应商变更审计', '<span class="tag">providers.audit.jsonl</span>', '每次增删改自动记录')
    + '<div style="margin-top:14px;padding:10px 12px;border:1px solid rgba(226,88,78,.4);border-radius:8px">'
    + '<div style="font-size:12.5px;color:var(--c-err);font-weight:600">应急 · 恢复官方配置</div>'
    + '<div class="sub">清除本软件写入的全部托管痕迹(config/auth),~/.codex 回到官方初始状态;操作前自动备份,可从备份找回。</div>'
    + '<button class="btn sm danger" data-a="restore-official" style="margin-top:6px">执行恢复</button></div>';
}
function setAboutHtml() {
  return '<h3 style="margin:2px 0 4px;font-size:13.5px">关于</h3>'
    + '<div style="display:flex;align-items:center;gap:12px;padding:10px 0;border-bottom:1px solid var(--hair)">'
    + '<img src="brand-logo.svg" alt="2xapi" style="width:48px;height:48px;border-radius:12px;object-fit:cover;flex:none">'
    + '<div style="min-width:0"><div style="font-size:13.5px;font-weight:600">2xapi Codex Console <span class="tag">v1.0.0</span></div>'
    + '<div class="sub" style="margin-top:2px">让桌面版 Codex 一键走中转站</div></div></div>'
    + setRow('检查更新', '<button class="btn sm ghost" data-a="about-update">检查</button>', '')
    + '<div class="sub" style="margin-top:10px">本软件为专有许可(Proprietary),仅供授权使用;Codex 与 Claude 的名称及图标归其各自所有方。</div>';
}
async function doRestoreOfficial() {
  var yes = await askConfirm("恢复官方配置?", "清除本软件写入的全部托管痕迹(config 托管段 / auth Key),~/.codex 回到官方初始状态;操作前自动备份,可从 备份 找回。");
  if (!yes) return;
  state.busy = "restore"; renderSettings();
  try {
    try { await api.snapshot(); } catch (e) { /* 备份失败不阻断 */ }
    var r = await api.desktopUnhost();
    await refreshAll();
    showToast(r && r.restored ? "已恢复官方配置(已自动备份)" : "当前未托管,无需恢复", "ok");
  } catch (e) { showToast(e.message, "error"); }
  state.busy = null; render();
}

/* ── 事件(单一委托,无内联 onclick)── */
document.addEventListener("click", function (ev) {
  /* 点账号菜单外任意处收起(含非 data-a 区域) */
  if (state.menuOpen && !ev.target.closest(".user-menu") && !ev.target.closest("[data-a='user-menu']")) {
    state.menuOpen = false;
    renderTopAuth();
  }
  var t = ev.target.closest("[data-a]"); if (!t) return;
  var a = t.dataset.a;
  if ((a === "settings-close" || a === "imp-close" || a === "login-close") && ev.target !== t) return; /* 点遮罩关闭,点内容不关 */
  switch (a) {
    case "agent":
      state.agent = t.dataset.g; state.view = "dash"; state.diag = null; state.test = null; state.search = "";
      render();
      if (state.agent === "claude") refreshClaudeState().then(function () { render(); });
      else if (state.agent === "hermes") refreshHermesState().then(function () { render(); });
      else if (GW_AGENTS[state.agent]) refreshGwState(state.agent).then(function () { render(); });
      else state.claude = null;
      break;
    case "view":
      state.view = t.dataset.v; render();
      if (state.view === "history") {
        if (state.agent === "codex") { loadSessions(); loadSessionsSettings(); }
        else loadClaudeSessions(true);
      }
      if (state.view === "eco") loadEco();
      if (state.view === "plug") plug2Load();
      break;
    case "sel": state.selId = t.dataset.id; state.diag = null; render(); break;
    case "cap-probe": doCapProbe(t.dataset.id); break;
    case "eco-agent":
      if (ECO_STATE.agent !== t.dataset.g) { ECO_STATE.agent = t.dataset.g; ECO_STATE.data = null; loadEco(); }
      break;
    case "eco-install": {
      var pp = (ECO_STATE.presets || []).find(function (x) { return x.id === t.dataset.id; });
      if (pp && pp.params && pp.params.length) {
        ECO_STATE.pendingInstall = t.dataset.id;
        ECO_STATE.pendingParams = {};
        render();
      } else doEcoInstall(t.dataset.id);
      break;
    }
    case "eco-param-cancel": ECO_STATE.pendingInstall = null; ECO_STATE.pendingParams = {}; render(); break;
    case "eco-param-do": {
      var vals = {};
      document.querySelectorAll("[data-pk]").forEach(function (inp) { vals[inp.dataset.pk] = inp.value.trim(); });
      ECO_STATE.pendingParams = vals;
      doEcoInstall(t.dataset.id, vals);
      break;
    }
    case "plug-install": doPlugInstall(t.dataset.src, t.dataset.id); break;
    case "plug-op": doPlugOp(t.dataset.op, t.dataset.id); break;
    case "plug-src-toggle": PLUG_STATE.srcForm = !PLUG_STATE.srcForm; render(); break;
    case "plug-src-do": {
      var pv = {};
      document.querySelectorAll("[data-pk]").forEach(function (inp) { pv[inp.dataset.pk] = inp.value.trim(); });
      if (!pv["src-id"] || !pv["src-name"] || !pv["src-url"]) { showToast("源 id / 名称 / 清单地址必填", "error"); break; }
      PLUG_STATE.busy = "src";
      api.plugSrcAdd(pv["src-id"], pv["src-name"], pv["src-url"]).then(function () {
        showToast("第三方源已添加", "ok");
        PLUG_STATE.srcForm = false;
      }).catch(function (e) { showToast(e.message || "添加失败", "error"); })
        .finally(function () { PLUG_STATE.busy = null; loadPlug(); });
      break;
    }
    case "plug-src-browse":
      PLUG_STATE.browse = { sourceId: t.dataset.id, name: t.dataset.name || t.dataset.id, loading: true };
      render();
      api.plugSrcList(t.dataset.id).then(function (v) {
        PLUG_STATE.browse.plugins = (v && v.plugins) || [];
        PLUG_STATE.browse.error = null;
      }).catch(function (e) { PLUG_STATE.browse.error = e.message || "拉取失败"; })
        .finally(function () { PLUG_STATE.browse.loading = false; render(); });
      break;
    case "plug-src-browse-close": PLUG_STATE.browse = null; render(); break;
    case "plug-src-del":
      PLUG_STATE.busy = t.dataset.id;
      api.plugSrcDel(t.dataset.id).then(function () { showToast("源已移除", "ok"); })
        .catch(function (e) { showToast(e.message || "移除失败", "error"); })
        .finally(function () { PLUG_STATE.busy = null; loadPlug(); });
      break;
    case "plug2-tab": PLUG2.tab = t.dataset.t; PLUG2.detail = null; PLUG2.cfgModels = null; render(); break;
    case "plug2-cat": PLUG2.cat = t.dataset.v; render(); break;
    case "plug2-dt": PLUG2.dt = t.dataset.t; render(); break;
    case "plug2-open": plug2OpenDetail(t.dataset.id, "cfg"); break;
    case "plug2-doc": plug2OpenDetail(t.dataset.id, "doc"); break;
    case "plug2-back": PLUG2.detail = null; PLUG2.cfgModels = null; render(); break;
    case "plug2-install": {
      var id2 = t.dataset.id;
      var done = function () {
        PLUG2.installed[id2] = 1; PLUG2.enabled[id2] = true;
        PLUG2.tab = "installed"; PLUG2.detail = null;
        showToast("已安装:点「配置」填写 API / Key / 模型后即可使用", "ok");
      };
      if (PLUG2.mode !== "api") { done(); render(); break; }
      PLUG2.busy = id2;
      api.plugInstallId(id2).then(done).catch(function (e) { showToast(e.message || "安装失败", "error"); })
        .finally(function () { PLUG2.busy = null; render(); });
      break;
    }
    case "plug2-save": plug2SaveCfg(t.dataset.id || PLUG2.detail); break;
    case "plug2-update": {
      var id4 = t.dataset.id;
      var upd = PLUG2.updates[id4];
      var done = function () {
        delete PLUG2.updates[id4];
        var p4 = plug2ById(id4);
        if (p4) p4.version = upd || p4.version;
        showToast("已更新到最新版", "ok");
      };
      if (PLUG2.mode !== "api") { done(); render(); break; }
      PLUG2.busy = id4;
      api.plugUpdate(id4).then(done).catch(function (e) { showToast(e.message || "更新失败", "error"); })
        .finally(function () { PLUG2.busy = null; render(); });
      break;
    }
    case "plug2-toggle": {
      var id3 = t.dataset.id;
      var next = !PLUG2.enabled[id3];
      if (PLUG2.mode !== "api") { PLUG2.enabled[id3] = next; showToast(next ? "已启用" : "已停止", "ok"); render(); break; }
      PLUG2.busy = id3;
      api.plugToggle(id3, next).then(function () {
        PLUG2.enabled[id3] = next;
        showToast(next ? "已启用" : "已停止", "ok");
      }).catch(function (e) { showToast(e.message || "操作失败", "error"); })
        .finally(function () { PLUG2.busy = null; render(); });
      break;
    }
    case "plug2-uninstall": {
      var id5 = t.dataset.id;
      var done = function () {
        delete PLUG2.installed[id5]; delete PLUG2.enabled[id5]; PLUG2.detail = null;
        showToast("已卸载", "ok");
      };
      if (PLUG2.mode !== "api") { done(); render(); break; }
      PLUG2.busy = id5;
      api.plugRemove(id5).then(done).catch(function (e) { showToast(e.message || "卸载失败", "error"); })
        .finally(function () { PLUG2.busy = null; render(); });
      break;
    }
    case "plug2-ui": PLUG2.ui = t.dataset.id; render(); break;
    case "plug2-ui-back": PLUG2.ui = null; render(); break;
    case "plug2-ui-run": {
      var uir = document.getElementById("uiResult");
      if (uir) { uir.innerHTML = "处理中…"; setTimeout(function () { uir.innerHTML = "✓ 完成(演示):结果已返回并存入媒体暂存,可继续喂给下游流程。"; }, 1200); }
      break;
    }
    case "plug2-mup": {
      var st = PLUG2.cfgModels; if (!st) break;
      var i6 = +t.dataset.i;
      if (i6 > 0 && i6 < st.rows.length) { var tmp = st.rows[i6 - 1]; st.rows[i6 - 1] = st.rows[i6]; st.rows[i6] = tmp; render(); }
      break;
    }
    case "plug2-mdel": {
      var st2 = PLUG2.cfgModels; if (!st2) break;
      var i7 = +t.dataset.i;
      if (i7 >= 0 && i7 < st2.rows.length) st2.rows.splice(i7, 1);
      render();
      break;
    }
    case "plug2-madd": {
      var st3 = PLUG2.cfgModels; if (!st3) break;
      st3.rows.push({ model: "", ep: "", note: "" });
      render();
      break;
    }
    case "plug2-src-toggle": PLUG2.srcForm = !PLUG2.srcForm; render(); break;
    case "plug2-src-do": {
      var pk = {};
      document.querySelectorAll("[data-pk]").forEach(function (inp) { pk[inp.dataset.pk] = inp.value.trim(); });
      if (pk["d-json"]) {
        /* 粘贴 manifest */
        var m;
        try { m = JSON.parse(pk["d-json"]); } catch (e) { showToast("manifest JSON 解析失败:" + e.message, "error"); break; }
        if (!m || !m.id || !m.name) { showToast("manifest 校验失败:缺 id/name", "error"); break; }
        if (PLUG2.mode !== "api") {
          PLUG2.local.push({ id: m.id, name: m.name, src: "AI", detail: m.desc || m.short_desc || "" });
          PLUG2.srcForm = false;
          showToast("✓ manifest 校验通过,已上架到「我的源」(演示)", "ok");
          render();
        } else {
          PLUG2.busy = "paste";
          api.plugLocal(m).then(function () {
            showToast("✓ manifest 校验通过,已上架到「我的源」", "ok");
            PLUG2.srcForm = false;
          }).catch(function (e) { showToast(e.message || "校验失败", "error"); })
            .finally(function () { PLUG2.busy = null; plug2Load(); });
        }
      } else if (pk["r-id"] && pk["r-name"] && pk["r-url"]) {
        /* 远程清单 */
        if (PLUG2.mode !== "api") {
          PLUG2.mySources.push({ id: pk["r-id"], name: pk["r-name"], url: pk["r-url"] });
          PLUG2.srcForm = false;
          showToast("远程源已添加(演示)", "ok");
          render();
        } else {
          PLUG2.busy = "src";
          api.plugSrcAdd(pk["r-id"], pk["r-name"], pk["r-url"]).then(function () {
            showToast("第三方源已添加", "ok");
            PLUG2.srcForm = false;
          }).catch(function (e) { showToast(e.message || "添加失败", "error"); })
            .finally(function () { PLUG2.busy = null; plug2Load(); });
        }
      } else {
        showToast("请粘贴 manifest,或填写远程源 id / 名称 / 地址", "error");
      }
      break;
    }
    case "plug2-src-use": {
      var id6 = t.dataset.id;
      var done = function () {
        PLUG2.installed[id6] = 1; PLUG2.enabled[id6] = true;
        showToast("已安装", "ok");
      };
      if (PLUG2.mode !== "api") { done(); showToast("已安装(演示)", "ok"); render(); break; }
      PLUG2.busy = id6;
      api.plugInstallId(id6).then(done).catch(function (e) { showToast(e.message || "安装失败", "error"); })
        .finally(function () { PLUG2.busy = null; render(); });
      break;
    }
    case "plug2-local-del": {
      var i9 = +t.dataset.i;
      var mine = PLUG2.local.concat(PLUG2.mySources);
      var it = mine[i9];
      if (it) {
        var li = PLUG2.local.indexOf(it);
        if (li >= 0) PLUG2.local.splice(li, 1); else { var si = PLUG2.mySources.indexOf(it); if (si >= 0) PLUG2.mySources.splice(si, 1); }
      }
      render();
      break;
    }
    case "eco-jump":
      ECO_STATE.agent = t.dataset.g; ECO_STATE.data = null; ECO_STATE.pendingInstall = null;
      state.view = "eco"; render(); loadEco();
      break;
    case "eco-op": doEcoOp(t.dataset.op, t.dataset.id); break;
    case "accel": doAccel(t.dataset.m); break;
    case "user-menu": state.menuOpen = !state.menuOpen; renderTopAuth(); break;
    case "settings-open": openSettings(); break;
    case "settings-close": document.getElementById("setMask").style.display = "none"; break;
    case "set-tab": state.setTab = t.dataset.s; renderSettings(); if (t.dataset.s === "usage") doUsageRefresh(); break;
    case "usage-refresh": doUsageRefresh(); break;
    case "autostart-toggle": doAutostartToggle(); break;
    case "bal-toggle":
      state.balShow = !state.balShow;
      try { localStorage.setItem("2xapi.balShow", state.balShow ? "on" : "off"); } catch (e) {}
      renderSettings(); renderTopAuth(); break;
    case "ipm-toggle": break; /* 官方线路本期只读 */
    case "ipm-add": doIpmAdd(); break;
    case "ipm-del": doIpmDel(); break;
    case "ipm-test": doIpmTest(); break;
    case "ipm-refresh": doIpmRefresh(); break;
    case "login": openLogin(); break;
    case "login-demo": openLogin(); break;
    case "do-login": doLogin(); break;
    case "login-close": document.getElementById("loginMask").style.display = "none"; break;
    case "logout": doLogout(); break;
    case "site":
      api.openUrl("https://2xa.cc.cd").then(function () { showToast("已在浏览器打开 2xapi 官网", "ok"); })
        .catch(function (e) { showToast("打开失败,请手动访问 https://2xa.cc.cd(" + e.message + ")", "error"); });
      break;
    case "import-keys": openImport(); break;
    case "imp-close": document.getElementById("impMask").style.display = "none"; state.importBusy = false; break;
    case "imp-do": doImport(); break;
    case "edit": openEdit(t.dataset.id); break;
    case "new": openPresetPicker(); break;
    case "preset-pick": applyPreset(t.dataset.id); break;
    case "preset-close": closePresetPicker(); break;
    case "preset-manual": closePresetPicker(); openEdit(null); break;
    case "edit-save": doSaveEdit(); break;
    case "close-edit": closeEdit(); break;
    case "mfetch": doFetchModels(); break;
    case "mrow-add": state.edit.models.push({ name: "", contextWindow: null }); renderModelRows(); break;
    case "mrow-del": state.edit.models.splice(Number(t.dataset.i), 1); renderModelRows(); break;
    case "del": {
      var delp = lineOf(t.dataset.id);
      if (!delp) break;
      askConfirm("删除供应商「" + delp.name + "」?", "将移除该供应商的地址与 Key(仅从本软件移除,不影响你的 " + (state.agent === "claude" ? "Claude" : "Codex") + " 配置)。此操作不可撤销。").then(function (yes) {
        if (!yes) return;
        api.deleteProvider(delp.id).then(function () {
          if (hostedBy(delp.id)) return state.agent === "claude" ? refreshClaudeState() : refreshDesktop();
        }).then(function () {
          return refreshProviders();
        }).then(function () {
          if (state.selId === delp.id) state.selId = providersFor(state.agent).length ? providersFor(state.agent)[0].id : null;
          render();
          showToast("已删除「" + delp.name + "」", "ok");
        }).catch(function (e) { showToast(e.message, "error"); });
      });
      break;
    }
    case "restore-official": doRestoreOfficial(); break;
    case "confirm-yes":
      document.getElementById("confirmMask").style.display = "none";
      if (state.confirmCb) { var cb = state.confirmCb; state.confirmCb = null; cb(true); }
      break;
    case "confirm-no": closeConfirm(); break;
    case "host": {
      /* 通用平台世界(genericDashHtml 按钮):确认后走泛化路由 host(固定 gateway 通路) */
      var m = GW_META[state.agent] || {};
      askConfirm("开启托管?", (m.label || state.agent) + " 将写入托管配置(" + (m.overlay || "指向本机网关") + ");已有配置零触碰;操作前自动备份。").then(function (yes) {
        if (yes) doHost(t.dataset.id || state.selId, "gateway");
      });
      break;
    }
    case "host-on": {
      if (state.agent === "hermes") {
        askConfirm("开启托管?", "Hermes 将写入叠加条目 2xapi-gateway(指向本机网关),已有配置零触碰;操作前自动备份。").then(function (yes) {
          if (yes) doHost(t.dataset.id || state.selId, "gateway");
        });
        break;
      }
      var hw = codexWayNow();
      askConfirm("开启托管?", hw === "direct"
        ? "桌面版 Codex 将直连选中的供应商(Key 写入本地配置,不经网关、无加速);操作前自动备份。"
        : "桌面版 Codex 将走选中的供应商中转,官方登录保留;操作前自动备份。").then(function (yes) {
        if (yes) doHost(t.dataset.id || state.selId, hw);
      });
      break;
    }
    case "unhost":
      if (state.agent === "hermes") {
        askConfirm("还原官方?", "移除写入 ~/.hermes/config.yaml 的叠加条目并恢复模型指针;操作前自动备份。").then(function (yes) {
          if (yes) doUnhost();
        });
        break;
      }
      if (GW_AGENTS[state.agent]) {
        var mu = GW_META[state.agent] || {};
        askConfirm("还原官方?", "移除本软件为 " + (mu.label || state.agent) + " 写入的托管内容并恢复快照;操作前自动备份。").then(function (yes) {
          if (yes) doUnhost();
        });
        break;
      }
      askConfirm("还原官方?", "清除本软件写入的托管配置(config 托管段 / auth Key),~/.codex 回到官方状态;操作前自动备份。").then(function (yes) {
        if (yes) doUnhost();
      });
      break;
    case "way":
      if (state.agent === "claude") {
        state.claude = state.claude || {};
        state.claude.way = t.dataset.w;
      } else {
        state.codexWay = t.dataset.w === "direct" ? "direct" : "gateway";
      }
      render();
      break;
    case "claude-copy": {
      var cmd = state.claude && state.claude.command;
      if (!cmd) { showToast("没有可复制的启动命令", "error"); break; }
      copyText(cmd).then(function () { showToast("已复制启动命令,粘贴到终端运行", "ok"); })
        .catch(function () { showToast("复制失败,请手动复制命令", "error"); });
      break;
    }
    case "gw-start":
      (async function () {
        state.busy = "gw-start"; render();
        try {
          var pid = state.selId || (providersFor(state.agent)[0] || {}).id;
          var r = await api.agentStart(state.agent, "gateway", pid);
          if (r && r.command) {
            try { await navigator.clipboard.writeText(r.command); } catch (e) { /* 剪贴板不可用时仅提示 */ }
            showToast("启动命令已复制,粘贴到终端运行(命令中 Key 为占位,真实 Key 在网关)", "ok");
          } else showToast((r && (r.hint || r.note)) || "已生成", "ok");
        } catch (e) { showToast(e.message, "error"); }
        state.busy = null; render();
      })();
      break;
    case "claude-start":
      askConfirm("启动 Claude Code?", "注入环境变量(ANTHROPIC_BASE_URL 等)并生成注入式启动命令,不动 ~/.claude 配置。").then(function (yes) {
        if (yes) doClaudeStart(t.dataset.id || state.selId);
      });
      break;
    case "claude-stop":
      askConfirm("停用注入?", "清除注入态,Claude Code 回到官方 Anthropic。").then(function (yes) {
        if (yes) doClaudeStop();
      });
      break;
    case "diag": doDiag(); break;
    case "test": doTestConnection(); break;
    case "sess-continue": showToast("继续历史会话:请在桌面版 Codex 里打开对应对话", "ok"); break;
    case "sess-repair": doSessionsRepair(); break;
    case "csess-refresh": loadClaudeSessions(true); break;
    case "csess-more": loadClaudeSessions(false); break;
    case "adv-recodex": showToast("已重新检测:Codex CLI 路径 /Applications/ChatGPT.app/…/codex", "ok"); break;
    case "adv-inspect":
      api.inspectHistory().then(function (r) {
        var stt = (r && r.state) || {};
        showToast("运行日志摘要:会话 " + stt.total + " 个 · 记录 " + stt.rolloutTotal + " 条", "ok");
      }).catch(function () { showToast("运行日志界面后置,可查看 ~/.codex 目录", "ok"); });
      break;
    case "about-update": showToast("已是最新版本 v1.0.0", "ok"); break;
  }
});

document.addEventListener("change", function (ev) {
  var sel = ev.target.closest("[data-a='prov']");
  if (sel && sel.value) {
    var id = sel.value;
    state.selId = id;
    if (state.agent === "claude") {
      if (claudeStarted()) doClaudeStart(id); /* 已注入:切供应商 = 重新注入 */
      else render();
    } else if (state.agent === "hermes") {
      if (hermesHosted()) doHost(id, "gateway"); /* 已托管:切供应商 = 条目热更新 */
      else render();
    } else if (hosting()) doHost(id); /* 已托管:切供应商 = 网关热切换 */
    else render();
    return;
  }
  var sauto = ev.target.closest("[data-a='sess-autofix']");
  if (sauto) {
    var on = sauto.checked;
    api.sessionsSetSettings(on).then(function () {
      showToast(on ? "已开启启动前自动修复" : "已关闭启动前自动修复", "ok");
    }).catch(function (e) { showToast(e.message, "error"); });
    return;
  }
});
document.addEventListener("change", function (ev) {
  var t = ev.target;
  if (t && t.dataset && t.dataset.a === "cap-set") {
    doCapSet(t.dataset.id, t.dataset.model, t.dataset.dim, t.value);
  }
  if (t && t.dataset && t.dataset.a === "plug2-local-file") {
    plug2LocalFile(t);
    return;
  }
});
document.addEventListener("input", function (ev) {
  if (ev.target.dataset && ev.target.dataset.a === "search") {
    state.search = ev.target.value.trim().toLowerCase();
    renderRailRows();
    return;
  }
  if (ev.target.dataset && ev.target.dataset.a === "plug2-search") {
    PLUG2.q = ev.target.value || "";
    render();
    return;
  }
  /* 插件配置页编辑:模型行 / 参数 / 故障转移直接落到 PLUG2 状态,保存时组包提交 */
  if (ev.target.dataset && ev.target.dataset.pk && state.view === "plug" && PLUG2.tab === "detail") {
    var pk = ev.target.dataset.pk;
    if (pk === "failover") PLUG2.cfgFailover = ev.target.checked;
    else if (pk.indexOf("m-") === 0) {
      var parts = pk.split("-"), i = +parts[1], f = parts[2];
      if (PLUG2.cfgModels && PLUG2.cfgModels.rows[i] && f) PLUG2.cfgModels.rows[i][f] = ev.target.value;
    } else if (pk.indexOf("c-") === 0) {
      if (!PLUG2.cfgVals) PLUG2.cfgVals = {};
      PLUG2.cfgVals[pk.slice(2)] = ev.target.type === "checkbox" ? ev.target.checked : ev.target.value;
    }
    return;
  }
  if (ev.target.id === "ipmNew") { state.nodeDraft = ev.target.value; return; }
  if (ev.target.dataset && ev.target.dataset.l === "email") { state.loginEmail = ev.target.value; return; }
  if (ev.target.dataset && ev.target.dataset.l === "password") { state.loginPassword = ev.target.value; return; }
});

/* 自定义浮层 tooltip:悬停立即显示,替代原生 title(插件市场图标按钮用) */
var ttEl = null;
function ttShow(el, text) {
  if (!ttEl) {
    ttEl = document.createElement("div");
    ttEl.className = "tt";
    document.body.appendChild(ttEl);
  }
  ttEl.textContent = text;
  ttEl.style.display = "block";
  var r = el.getBoundingClientRect();
  ttEl.style.left = Math.round(r.left + r.width / 2) + "px";
  ttEl.style.top = Math.round(r.top - 8) + "px";
}
function ttHide() { if (ttEl) ttEl.style.display = "none"; }
document.addEventListener("mouseover", function (e) {
  var el = e.target && e.target.closest ? e.target.closest("[data-tip]") : null;
  if (el) ttShow(el, el.getAttribute("data-tip"));
});
document.addEventListener("mouseout", function (e) {
  if (e.target && e.target.closest && e.target.closest("[data-tip]")) ttHide();
});

/* ── 启动 ── */
try { state.balShow = localStorage.getItem("2xapi.balShow") !== "off"; } catch (e) {}
/* ── 多平台导航数据驱动(「全部做好」批次):非静态平台按注册表注入——frontend_ready=true
 * 渲染可点按钮(品牌标/首字母彩色标,点击切换世界),false 渲染灰标「即将上线」。
 * codex/claude/hermes 真实按钮保留静态 DOM;拉取失败维持现状。幂等:只注入一次。── */
/* 品牌图标(simple-icons 官方原文,2026-08-16 拉取;仅导航识别用)。
 * 尺寸按 codex(19px 花结)的视觉重量逐一校准:星形视觉轻→放大,满幅粗图形→略缩。 */
var AGENT_NAV_ICON = {
  // Google Gemini 官方四角星(视觉轻,放大至 21)
  gemini: '<svg height="21" viewBox="0 0 24 24" width="21" xmlns="http://www.w3.org/2000/svg"><title>Gemini</title><defs><linearGradient id="gem-nav-g" x1="0%" y1="0%" x2="100%" y2="100%"><stop offset="0%" stop-color="#4285F4"/><stop offset="100%" stop-color="#9B72CB"/></linearGradient></defs><path d="M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z" fill="#3186FF"></path><path d="M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z" fill="url(#lobe-icons-gemini-fill-0)"></path><path d="M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z" fill="url(#lobe-icons-gemini-fill-1)"></path><path d="M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z" fill="url(#lobe-icons-gemini-fill-2)"></path><defs><linearGradient gradientUnits="userSpaceOnUse" id="lobe-icons-gemini-fill-0" x1="7" x2="11" y1="15.5" y2="12"><stop stop-color="#08B962"></stop><stop offset="1" stop-color="#08B962" stop-opacity="0"></stop></linearGradient><linearGradient gradientUnits="userSpaceOnUse" id="lobe-icons-gemini-fill-1" x1="8" x2="11.5" y1="5.5" y2="11"><stop stop-color="#F94543"></stop><stop offset="1" stop-color="#F94543" stop-opacity="0"></stop></linearGradient><linearGradient gradientUnits="userSpaceOnUse" id="lobe-icons-gemini-fill-2" x1="3.5" x2="17.5" y1="13.5" y2="12"><stop stop-color="#FABC12"></stop><stop offset=".46" stop-color="#FABC12" stop-opacity="0"></stop></linearGradient></defs></svg>',
  // xAI/Grok 母品牌 X(满幅细线,缩至 16 压视觉重量)
  grokbuild: '<svg fill="#D8DCE3" fill-rule="evenodd" height="18" viewBox="0 0 24 24" width="18" xmlns="http://www.w3.org/2000/svg"><title>Grok</title><path d="M9.27 15.29l7.978-5.897c.391-.29.95-.177 1.137.272.98 2.369.542 5.215-1.41 7.169-1.951 1.954-4.667 2.382-7.149 1.406l-2.711 1.257c3.889 2.661 8.611 2.003 11.562-.953 2.341-2.344 3.066-5.539 2.388-8.42l.006.007c-.983-4.232.242-5.924 2.75-9.383.06-.082.12-.164.179-.248l-3.301 3.305v-.01L9.267 15.292M7.623 16.723c-2.792-2.67-2.31-6.801.071-9.184 1.761-1.763 4.647-2.483 7.166-1.425l2.705-1.25a7.808 7.808 0 00-1.829-1A8.975 8.975 0 005.984 5.83c-2.533 2.536-3.33 6.436-1.962 9.764 1.022 2.487-.653 4.246-2.34 6.022-.599.63-1.199 1.259-1.682 1.925l7.62-6.815"></path></svg>',
  // CodeBuddy 官方机器人标(满幅,18)
  workbuddy: '<img src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAFgAAABYCAYAAABxlTA0AAAABGdBTUEAALGPC/xhBQAAAAFzUkdCAK7OHOkAAAAgY0hSTQAAeiYAAICEAAD6AAAAgOgAAHUwAADqYAAAOpgAABdwnLpRPAAAAAZiS0dEAP8A/wD/oL2nkwAAAAlwSFlzAAALEwAACxMBAJqcGAAAGudJREFUeNrNXXucHUWV/k7fvvfO3Hm/n8kkTEJgQkx4JhFJDAooBEFZHi4+EHFdECQIu2RxF5RVISCi67K6IKjo8tD9YRQjrvwUQYQJ4R0mDwjkNZNkMq/Me+bOvX32j35VVVf3vTMJ7FZ+ndvdVX266quvzjl1qu5cYmZMJ5U//wixQU0AnQTwQjCOAlEDwDUAlQJcDFABgATAJgADgAmAJEEcuKO/p0tqlUl4VpURKOsUYjAIGQAWGBkQ0mBMADQC4kEAvWDsA2EngC0AXgBz1+CyS6aFF+ULcNkLv6gF8xUALgHQ5gAXDgAJ50B+wOlADgM9387It465ZDEsEDrAeJTBPxpadsnBfF6TE+CyF36RAvONAK4DUOxWRlunMPYEmMo2k3KBHwZ2GFtzACddRr3brV9YOeYREN0NYN3g0ovHZgxw2cZHlwL8cwZa7cJOJcmuA8i+B+XaK6O+LAdWYjl23+dcuPJI6WDpOefdXj1ILh8AzK0z8tAsukcZb4Hos4NLL26fFsAV7Y/AIlzOwD0AknLvInwIH+6w1YGgy6M8ryPq05IowuxkEbaMD6IvMxkuL+z9frlJAq42GPcPaPRzAOCK9keQBa8B4S5PRGgjPIOhp6NaRgRNHAoqhaIaqVI9rH66DnLKrSqtx03Ni1BgGDRhWXxf91v4Zd9usCg8VA0JdfbLMYGuNxjfVUEOGKos8eclcMUXBsiuqVCgSIgKIuchEq8157qxGsZs1pRh4ZOAk4qqcPOs96HAMIhAKDRidG3DMXRDYxtiomBdvVxBwboSg+/Kgi8PNFNkcFn7w8sYeArgpCdlJkM+l8t0WDIdIZ5Szl9ElZnAj+edikoz6Y0tcs4IwO8Guvibna/Dmm6d/PMJAlYNLv3kRreIx+Cy9odSDP4piJNen5BLC83BIdfifVLuU0hZFo6w97iyRDcgV/0Uedc1tqHSTBIBMMgGlxxwCcDqima6oXFheB3U+lGgfgUM/Ly0/eFUAGAG1oIwXxqRLBMvyCZI6hUeLZTnxTKEALs57DkgMBzFe6L2UevNJJdbXlKDD5bWEUnA2uAaDoeJgAurW+jy2nkBlSjVRVVH0qu5FcQ3SgDXfCBey8Rr2HmSiW2F736CIZ0591lguXdP/CeOAlWWUB6keU56L6s1CDwH5XnxOm4Qrqk/BrbWVYEFiMhmtAP0lfUL6MPlDRIO4GC7dO113ntd9WmJWg/gyW//5AoAxciZpjetllitSpihqJmkcyqaMaegmAgEQwQWBCJfRRgO8DEQvj5rCR1dUOpXNcyw6lPx5Ld/fAUAGKXtD4Pt6e8RBpejnzkS/nIe9UuSgctq5km6lsgBluAALrCa7PyimIm75pyEkpg5szYwX0LFKTIAbgJxW6ixkIDKVUYtP91npnFwfvU7p6IZDYlCMuDoXoGxHou9PNnozUmW0M3Ni4XJwLTa3xZfeWKTAc6egqjAzXuZpjNI8hgBcTLw6ZpWD0ADqjogD1hD1c0Ow8+uaKYLqlpmUj+D5jSdZFgGtU2HqZzjevosV9yewxxJYn1WldWjOZEiIpJVhAOsaNjkPJL09NqmRTQ7UZRn/fw6Gi31Cw0Cz42CDEqToBPlup25ms+qHA6XqZ6zXD9ElHfTp6pbA2AZLmvFa5fVpM8rjpm4veVEGMKwie5uryZHGczcGMX/HKbKGQ4s+7Sh5fwCauch5JpDSuV6fnGqEm2pclJZq6oDH1jFJ4ZT1mH/ScXV9OmaVi3xwupntDQ0GCBUz8yIHY4BnMFB03vHRdVzhQmE7eeK4Pl6VlEPIO0sjwB8pXEhNSdSeWPAQI0BcJnUAXlRNkdi5VNMRzKcGZKqzQJ8qKyBSFEHqqvmqw7BAJJGdTidVByL4+bmJXnXLzanvtQAUCQZGHLPgz2SP9umY7RmMCpyyD23YhaSRszzc2V1AC1rfaMXNHiinDPLm2hlaX1e9WOg2ABQOCOavOtpZu83QDi/skUAlhR1QJq8EIaHsP+fmxfDpLyGYoHBQCJMcYeN9KjyERY1ICsoJ6ifdB5NVF2WFFVirjAtdicXojpQJxeqJ2F4DCdJdbjG8JjCcrqwak5k/ZzrhAGwKTpAYuBE/RTzw8rrhnqYrKAcBN4GTbmouny8skUGMGRyYSisNbQGT/AylMjb9Y2LUGjEQuvnXMfd2Me7nt4LhVNlJnFmebMwJSYpYuZPhxHMA/SqQ8rzn2tKpOixBR/Gh8oao+w2Uar9gSxCbbu6bpsrP1f56cjOT0Z5LIFVZQ04s7wZy4prKWm4g1r2f8Vr/9wGUJtH5IEi5Qmjws17cbSXb9n7EjaN9Kj1Y0q1P5D3Csn/lxQnA6eV1OP8yhacWlpPBUbMg0Yc4j4gap6dESgLeQlJB6zUObCFEwCLGQ/1vs23dr6MfmGV+v8YYAZAsHcxUWQZAKiNF+DCqqNwQeVc1MYLyQdEZRUJwCp5ArAikFD0tShHlJFLTvfUOK/Z9Tx+f6jTBfj+nAC7TZzOJ4TKRMmJKuemuckSXF67AGdXzKYkxYRGqqwKqgMVkECeZsjrgM35DkEOM3Dfwa08aVmgwjwAjkrT1aTTKd+SKMaV9W34aPksMp2ogm+k3ObPcMiHdJDfcbk7T9/Jcp4ZbG4+EPhlpmum8ilfHkviyro2XFR9FOLkOkkIGa56daCCLuUpoHt5Gn0d1amygRTyXErD3lY6AwjeHafLAOG8yjm4rmERVZpJrZWXhmtYXgiwIJ2cCH0dpoNJ08kCAUQFF7HgpGrCI5mCsmcninFL84m0tKQ2pyEJZY4GdCB6yE9YWWwbO8TvTAyhZ2oCE5xBkmKoNAswJ1mMRakqKjcTghwKdIZP2CBeZjQb383pgS3bAOHS6vm4tuE4KjRMv+K64Ro25JU8lVWqjhzMTvH6/p14fGA3No30IM3Z0FoaIF6UqsTqihZcXNWKOckSkmRqWCsmKmy/z5oOV48kr2vjhfjmrFOwvKSOPIYJ+i7KuQ/PC9ezg9k039e9Ffcf3IbBbHra9TXJwMcqWnBT0wk4NlVO+SBBBe33Sl5EmIlT3Srd/C3qWSh5y4vrsK5lGarNAgqyM1wd5Ov4i3kZtvjBnjdx9/7X0ZuZ0LZHrV8UFkmK4Zr64/DV5hORNGKRKAcAfrcTAfh0zdG4oWEJmUqAJWDZSW9Ioi29DywA/Gmoi7++90W8OTF4xNtyQlE1Hp5/BmYli0NBFgBWB/+RVxwmEb7adCIurmql0Lm+AGwgT6cOQly0HRND/LXOTfjjYNc0LElUO/R5zYkirF/wEbSlKrUPUkH7f3oMTlIMS4qqsaCwHHXxQhTH4khQDMyMKbYwYWUxYk1hMJvGocwkejMTODg1ju6pMYxmM7AimlJkxHHXnPdjZWkDBdiHmakD3WxrKJvmu/e/jgcObkWa35vB2RgvwpNt52JuQWkAZM9NO69iLq6sX4jSWIL0DdTMegT2DGbSODA1xnsmR7B7chg7J4fwzuQQdk+OoNgwcc9RK7AoVUn5GqTpOv5ZMD/SuwPrul5Gj6Nn36u0b2oUl7z1JP7cdh4Kla1WVND+Q6vIiGPDMedQ0ojlbKBq6XP5oGnOcoaZSmJxKS+X4y91gAKsmvfcSDd/be8LeH2s712AL9fM1s//cv37cHvLconFJgMoicXhghsZMAltfPikoMAwhREh5CmszemSaTpjb3qEv9H5Ih4f2PUueuy5JPv5P+h+AxdVzeMTims8kA0AGMqmkWWLvag/1M0ZJO16EdewSFrDcsq6QJG4XdRZKSC3M0jZRpp7Vdd976iVwZ37XuGVHb/Cb/IC90jtQ4i+P8UWrt/9LCxhZ7gJMEatKeyfGsOcZAkAhUEhUSpowACmx8DpjggAeKz/Hf5W14voSo8eIZCOLMgbR7rxcO+bfGnNAgKE9bjNY30Oq0hipsQyiMzUMZCCDHTzoOR5I4IiRoT/zCujPXzutg181c6nJXCPpGo4nBmqWI+b927EcCbNDsD2CugLI90egOqQ9wweKepBUh3kqA+9OpDyPNXhy/L2LkBWQ91TY7xm11949bYN2DTajeDqrbx+HXVQjiNXmWjZPtD7p0Zxx76XAQhu2nPDB5AFI+7sIcwraD0DdRD0SqCdJk9ylu/r7sD3DryO4Yi4AYVw2GdjNMdzsZYjy5JQhqUy93S/js/VHsumOz8ZzE5i00g3n1rSEBrgjgZKHweI9Eo0wDKAJw7t5ls7N2Hn5FAewLDmXn4A6ueturMw4GVQxc6YsDK4ac/ztpFzMzYM7MJpJY3T9081nRGcFESDDgBbxwdwS+dGfmaoKy9Q5Ht6oMMAihFhZWkzjktVYSibxpOHdmOfp9vZn0Rx/ksQggsABvD4wDtywP1PQ50Yyqa5XPwmZA6g1M4ITgrC1YErqz8zwXfuexk/692OTMT0VmSty3YK5KmgshCztfPq4kV4aP5HcVJRHbmTo3Erg6/u+Svff3CzDCgpDJXChsHuY4XVUsB9zJrCbwZ24rM1x+QASp7NhQdqotVBhi082LuN7+h6GYey4dNbCrkmwbhIAJO+LGAbzwfnnYWTi+slsYWGiW+3rKDO9BD/4dAuBTRBFtndS6TsulNo7l4Gtk093LcdFljYXahMCpRt9p6HIIUeBZdM45UYADaOHOAzt67nm/Y8h4HsRM5Nfm61ZWsvjgyAiO3DyTekT7vc0uJ6LC9p1GoOgwjfmv0BFJABw3lOPlwPh4VP5/DeDekIALx7cghPDXayNCtzzxU3KnxvFzQumn02Yk3hhj3P8vnbf4uO8f68AYWGrQR7L64PqtiwIMgGGAtTlZFvbC0opy/WvU8AD8JslLVy/bYLnUxuvmY35I8Odsg9oZtCuxMOhbWhU2EAOyYO8Vlb1/ODPVud0KbeT1UBVZnrlfVGUhBI8YgJnwOZ8UiACYSvNJ6M+kQKMWLEyIJBVgijZbDVcxBg6NzmV8d68Nfh/exuWhYnD546gA+sOvmQ1YHdGVvG+/m87Y9jx8Qh5E7hzJVYIg1/teHwr8k/nh3ag0M5wpllZgGtbVomdAxgECMmyJHVhgy4OIq0DGYA3z/wmuut+GwkvToQNyf76sNn+a6JIf6bNzegNzMOvaYNYy4gzaacrzeQct9tqMQuBQwXrJHsOL63b2NOz+uT1QtpSarWec6SRkFMI9/Q1MNREUH2AIyXRrvxl+EulmITUNQB9HrWi1mAMGFlcNnbT6InMDSj9obLhgyePHHqGmStCGwM8iGC87OeV7FjvC8S5BgZuGX2SpgE5zkfZEOR7TM7WEdDbph8dee+l2y3L2RLvfolP9W4GQBu69rEHeN9EnRqilIHqhGTmSqrDPurWiF6mPwjw1O4rfNp5PqTZkuLm+jcivnSs2KHBUYNLIHRARWhsorxxngvfj3wNod9OS8YDJLvd4z3870HNyOohoJqyQVVMnYadRBQDWQJjbZkdhHD9ICxYDosNMF4ZvBtPDO0MxJhIsLa5hUoopjPYrIEsC0ts202Wx7QGv766Y59L2LCyoarA4LDnGAk7PauTZjKsfAogqp+JUZVB6K+jREctkBoHDxgRbaZzuGdO97BnZ1PIW1lIkFuTpbR+0tneTJEWfZ74L1L9lq8+tlhP104jsHoTA/jvoObOVQdQA/6rskh/p/BXVqmBkOLGlAFF8w1Fp7FJpmtHmtddsFlrHLfZbSTt3uiB4/2vIJcqTlRohkNLtCWDLZXH4fBUV/Bcu//e/dr6J4aY1cHB31dOWZMIDzSp4srhIPq6l9Ppzr3ZyVK8LmaRfhy/YlYVTYbcaKAG6YyNgYfDJN8MOJOw+MCk+8/8Cz6p0YjWXwgPYC4ow5MQXYsYEz9jnRHlok8do0MZ9O4Y98mfLflg9pYgy6S9sShnSHS3K8LqO4YAlPNL9Ytxq2zVnibAgFg08g+/sKO36BnasTRwVCNCgwSO8CXLX9XzsZ0LDuGe/c/jbWzz9bWdvdEH786shumE3uwwGC2v/ZggWCRfY/YJpvlvQ8gBhsAZ3IbIcZDvduwa3KIJT0rMFZ04QazaWwe680xLmT2qvc+Uj4X62avksAFgJOLG+ne1tWIC7pPZo9ryHy2mSSw1jF2cTDiZB+/7XsZv+59iVWvondqGDfvegzMU568OJxRQcI7RFbLLmPGRLh9k5IFxnA2ba93hMV4nYuOsT5Wd/nIcYRo9gKMq+tPgBHyddVTSprpgqpj+Vd9b3izLG/GFcJqEpgtsth9w3f3bsDTAx18WvkxSMWSeHu8G0/0vYbB7ATi5FsliwmWA5rLXvseIwty2kHIMoNAlgnwFIBEPiAXx+Lhu23gs3nX5KDSb14AVdJH3nIP+deur3tsYXVkXf6xaQX+eGg7JqxJ26vwvAvRD5U9DX8GyoJa8+uzeXQnNo/u9OrJAOLeuQMsOapBANsiIAuA2FYRWUcwMTIGAG/BSzY9wRSnmOQ1hC1u9gdiu6KvK6sEVSe65/05gjJ1iRK6qn6Z5HqJBs0kRty7doY3+cM7Dv+wVYZ/eCrEk+uoGWJJXkBVCGrCtI1v2mBgQjfVgHLt8k+c0anehOumqd4DSdIgqQL3q//yhAL4df925EqfqTsZc5Plgt4VABFAdQ9TBJUcMF3ghCPuAWopHSHki34x+S6bC7xjG8YNgEfCDJv6ZexJzgaMWyCKRkDKMEPkyN0m60IBZGL8sHsTDqRHIgEuMOJ0XdPpMAGXMYKPCsUQ2YBJTCcfOBVIH1DLZyoJs0GPvXLHirPIGHjUADCEPNNIdkrzJ7DIixc7/6EuXqRlsB/vdZFVA+W+ezWancTtXc9wrnjB6eULaGlJi+3zBtSFFWi8ZPkhM9Y94ip40IwIoQMC73bOzxquHzQA9OTjpsHRi7pZG0kwAkcXVEQyWGSsOHtzo2Hu+WN9b+DV0f2RABMR1jSfgSSRrwfdWZyqLiCrAj9PAd0FNFBWmHIrOj8mxERixFhduQiX1S3vNQCOboGQuqfGfHVA7gRDtMN2aiusokIjrhhKobPIvyexlyCpCkYW3+j8I1s5WNxaUEfnVC2WDIyvF+X4g46VsRAgdWyPOZ0iGLIAe8+oWIh/mr2a3ty2fZ/BwDu6Sotrpgx71XV5SaO3VUoHrJviRgwrSpokxqrSdapBVBGuq/XKyF78tr8jZ9Tr7MrjNepAteyQDWEAUDkKZxKC8qBhtMPeOIBL607Fv7R8guL2BGmXAUKHut/L3/PlD++7W1ZhUaqaooAV0wVVRyPcJ9GxWglHkh9Q/86+pzCaTUeCXJsokyNopLDLBUIIBonRNt25VEYZAYIhgwlGc7IC32r9DC5v+DAZZAcpt23b1mGwlX3R8Y9D08LCalxWcxzlA6ybLqxcgBrT/UPQAmPJ94VD2SsGzgnoTg/ix93PRb6vOz2gWnAlrKioDMAOb4qBGzHiRkL4UwRfkZWKmbi4bhX+bcE1tLik1d+yBGS3bdv2gkHMXQBv0ccL7OPS6mMRo+n95ZmiWILWNJwoM5Z8+ZJrRjqw5QXFnx98Hp2TA1oWMzOe7NvkD28E3CXngBQJM0XDpDvIN3S6aN2ysoW4c/61uKj+Q5Qw4nJbgS3r16/vMrLLb2QAjypVlq5OK2meFrhuuqrueDQmiv3O8jw0NVypW7VwI2D2MWmlsXbnL9A/NSJvqGHG7/o28tMDrwjBFujDicICqFcOwTU2P3gPZYXELt9SUI9/mHsFrpl9KdUmK0h0RIX0yODgoP0rBMbGO2sJvAOgwF/BJgC7jv97NCVK8tcPQvpJz2b+4jtPSEz14gTww4suw9yhKfmU5BuoKrMAq6uWYH5hHUaz42gf3IJtozsli+4C4a8sKKFLyEEfGRcnWsLBXRtFZhHOrj0Lp1YsdfSstKlKTMMAzyPQQe9nHmIb7/g6gH9RS8bIwL7jr6KqeAozSVm2cGrHz/jV0QPCX9DzgzE+qyCxRAQ2LgAdh2L5CYpbJoYx3c6T36kGe0QyuRC7nwwgBhPvr1yOs2rOQMosIrcT9CEsAMC/EnAzIK8qr2PgbVULZ9nCeI51q6gUIwN3zF4lhB5l9wwIumqG50XIW6AC+w8Cq8iQV3WJhWEObyLj/qFQcSS5qkFa9wPQVrwAN7Reh483nEcpM0XigoF+wyzeBHC7e+EBnF164xjAnwJ4Qpx9uetyh5NWlMyiT1QuEHSvauycapIMvgu0u+lF/Ays4kLW2YENIdpNIvLuHJHldckafG725bhi9heovqBOWMeRoVXSBAGfJcD7hS7JNbCW3rgR4KuhWLnXxvL66bTwRIRvzlqJlGFKBo6EP0JKIuAkb4lSl+pJAFC3V0IOtIs+dfhGPvd+KlaAc+rOxbVHXY+2koUkxvxJOnMVhF1vi7PWeHb8agDSL3MFfC9mPADG9RB842eG9x4ewABakuW0pn6pD6LOXZPu67ZHie6buEohu3Wq2yezFJIMg+wOiBHh5Ipl+Mq8tVhRvYrihiksA9gk8DWtC6x9NZwZth7Y+8vrv7T51vsD3NJFq6j9dgD4PBG+D6CgyizEnuOvQoERn5En4aaxbJpPfuNH2Dt5KGjgyHX+g8bNWz9TDF1cKCtNJAiS3OCKhj96CMCcVCvOrjsfTYXNGr66wMrXLs47xt6Z+K/Ox77UPd7/wA+W3BbEMiocSBvXLSPwTwHMf2Teebiw6tjDAhgANgy8yX/71n+DJHcKcrxA8RriwqcXMhTLeS4aFIBtg+YaOA9gh+Hl8Qp8pPZjWFi6WPjzCr4BkxvrctdVCRb+1PvnHb/v/cOn7jp23caw9ub8yUlqvz0F4rUnpOrXbDrushKiw8SYGZe9/Ste379Vno5K/i+0bFWBdu+7MzTVB/YXRCGxuCCWwGlVp+MDVacjYSRIZCUJQLpaVta9jIOTB4cf795w9xuDb6y7e9F3Zv6Tk2Iy2m+r3bL4765YUFjl/mhqbKYY92fGeFXHA+hMH5IYbCoMlhkrqgnI/rCiXtxOM8hfDLX1MGFJ2Qk4q241yuMV5MMnf7rg+qACAGcB2jI0Nfjof+y55761rTcdmR9N1ZIQaAJwCoCFAOYAaARQDaAcQBEA52d/If7sr2RQt4wd5I9tfxBDmXFvWPsTDEjARqkKsUPUSUZMYO/swlk4t/58tBTOdQehxUDGQXmKgTQBEwBGAQwC6AFoP+xwbgeAFwF0Ic9tDm76Xxw38gTQJXnrAAAADnRFWHRTb2Z0d2FyZQBGaWdtYZ6xlmMAAAAASUVORK5CYII=" width="19" height="19" style="display:block;border-radius:4px" alt="CodeBuddy">',
  // OpenCode 官方标(满幅粗环,缩至 17)
  opencode: '<svg height="17" width="17" viewBox="0 0 240 300" xmlns="http://www.w3.org/2000/svg"><title>OpenCode</title><g clip-path="url(#clip0_1401_86274)"><mask id="mask0_1401_86274" style="mask-type:luminance" maskUnits="userSpaceOnUse" x="0" y="0" width="240" height="300"><path d="M240 0H0V300H240V0Z" fill="white"/></mask><g mask="url(#mask0_1401_86274)"><path d="M180 240H60V120H180V240Z" fill="#CFCECD"/><path d="M180 60H60V240H180V60ZM240 300H0V0H240V300Z" fill="#211E1E"/></g></g><defs><clipPath id="clip0_1401_86274"><rect width="240" height="300" fill="white"/></clipPath></defs></svg>',
  // OpenClaw 官方龙虾标(LobeHub 官方图标库,2026-08-16)
  openclaw: '<svg height="19" width="19" viewBox="0 0 120 120" fill="none" xmlns="http://www.w3.org/2000/svg"><title>OpenClaw</title><defs><linearGradient id="oc-g" x1="0%" y1="0%" x2="100%" y2="100%"><stop offset="0%" stop-color="#ff4d4d"/><stop offset="100%" stop-color="#991b1b"/></linearGradient></defs><path d="M60 10 C30 10 15 35 15 55 C15 75 30 95 45 100 L45 110 L55 110 L55 100 C55 100 60 102 65 100 L65 110 L75 110 L75 100 C90 95 105 75 105 55 C105 35 90 10 60 10Z" fill="url(#oc-g)"/><path d="M20 45 C5 40 0 50 5 60 C10 70 20 65 25 55 C28 48 25 45 20 45Z" fill="url(#oc-g)"/><path d="M100 45 C115 40 120 50 115 60 C110 70 100 65 95 55 C92 48 95 45 100 45Z" fill="url(#oc-g)"/><path d="M45 15 Q35 5 30 8" stroke="#ff4d4d" stroke-width="3" stroke-linecap="round"/><path d="M75 15 Q85 5 90 8" stroke="#ff4d4d" stroke-width="3" stroke-linecap="round"/><circle cx="45" cy="35" r="6" fill="#050810"/><circle cx="75" cy="35" r="6" fill="#050810"/><circle cx="46" cy="34" r="2.5" fill="#00e5cc"/><circle cx="76" cy="34" r="2.5" fill="#00e5cc"/></svg>',
  // Anthropic 星芒(与 claude 按钮同 path,导航识别用;视觉轻,21)
  "claude-desktop": '<svg height="21" viewBox="0 0 24 24" width="21" xmlns="http://www.w3.org/2000/svg"><title>Claude</title><path d="M4.709 15.955l4.72-2.647.08-.23-.08-.128H9.2l-.79-.048-2.698-.073-2.339-.097-2.266-.122-.571-.121L0 11.784l.055-.352.48-.321.686.06 1.52.103 2.278.158 1.652.097 2.449.255h.389l.055-.157-.134-.098-.103-.097-2.358-1.596-2.552-1.688-1.336-.972-.724-.491-.364-.462-.158-1.008.656-.722.881.06.225.061.893.686 1.908 1.476 2.491 1.833.365.304.145-.103.019-.073-.164-.274-1.355-2.446-1.446-2.49-.644-1.032-.17-.619a2.97 2.97 0 01-.104-.729L6.283.134 6.696 0l.996.134.42.364.62 1.414 1.002 2.229 1.555 3.03.456.898.243.832.091.255h.158V9.01l.128-1.706.237-2.095.23-2.695.08-.76.376-.91.747-.492.584.28.48.685-.067.444-.286 1.851-.559 2.903-.364 1.942h.212l.243-.242.985-1.306 1.652-2.064.73-.82.85-.904.547-.431h1.033l.76 1.129-.34 1.166-1.064 1.347-.881 1.142-1.264 1.7-.79 1.36.073.11.188-.02 2.856-.606 1.543-.28 1.841-.315.833.388.091.395-.328.807-1.969.486-2.309.462-3.439.813-.042.03.049.061 1.549.146.662.036h1.622l3.02.225.79.522.474.638-.079.485-1.215.62-1.64-.389-3.829-.91-1.312-.329h-.182v.11l1.093 1.068 2.006 1.81 2.509 2.33.127.578-.322.455-.34-.049-2.205-1.657-.851-.747-1.926-1.62h-.128v.17l.444.649 2.345 3.521.122 1.08-.17.353-.608.213-.668-.122-1.374-1.925-1.415-2.167-1.143-1.943-.14.08-.674 7.254-.316.37-.729.28-.607-.461-.322-.747.322-1.476.389-1.924.315-1.53.286-1.9.17-.632-.012-.042-.14.018-1.434 1.967-2.18 2.945-1.726 1.845-.414.164-.717-.37.067-.662.401-.589 2.388-3.036 1.44-1.882.93-1.086-.006-.158h-.055L4.132 18.56l-1.13.146-.487-.456.061-.746.231-.243 1.908-1.312-.006.006z" fill="#D97757" fill-rule="nonzero"></path></svg>'
};
var GW_NAV_COLOR = {
  gemini: "#7B8FF7", grokbuild: "#D8DCE3", opencode: "#C8CEDA", openclaw: "#FF6B5E",
  "claude-desktop": "#D97757", workbuddy: "#3DDB84"
};
function injectNavAgents() {
  return api.agents().then(function (reg) {
    var statics = Array.prototype.map.call(document.querySelectorAll('.nav .nav-btn.agent'), function (b) { return b.dataset.g; });
    var anchor = document.querySelectorAll('.nav .nav-btn.agent');
    anchor = anchor.length ? anchor[anchor.length - 1] : null;
    if (!anchor || anchor.dataset.navInjected) return;
    var rest = ((reg && reg.agents) || []).filter(function (m) { return statics.indexOf(m.id) < 0; });
    if (!rest.length) return;
    anchor.dataset.navInjected = "1";
    var html = rest.map(function (m) {
      var icon = AGENT_NAV_ICON[m.id];
      if (m.frontend_ready) {
        var color = GW_NAV_COLOR[m.id] || "#9CB4DE";
        var mark = icon
          ? '<span style="display:inline-flex;width:19px;height:19px;color:' + color + '">' + icon + '</span>'
          : '<span style="display:inline-flex;align-items:center;justify-content:center;width:19px;height:19px;font-size:11px;font-weight:700;color:' + color + '">' + esc((m.name || "?").trim().charAt(0).toUpperCase()) + '</span>';
        return '<button class="nav-btn agent" style="--ac:' + color + '" data-a="agent" data-g="' + esc(m.id) + '" title="' + esc(m.name) + '">'
          + mark + '<span class="tip">' + esc(m.tip || m.name) + '</span></button>';
      }
      var g = (m.name || "?").trim().charAt(0).toUpperCase();
      var grey = '<span style="display:inline-flex;align-items:center;justify-content:center;width:19px;height:19px;font-size:11px;font-weight:600;color:#8a8f98">' + esc(g) + '</span>';
      return '<button class="nav-btn" disabled style="--ac:#8a8f98" title="' + esc(m.name) + '">'
        + grey + '<span class="tip">' + esc(m.tip || ((m.name || "") + "(即将上线)")) + '</span></button>';
    }).join("");
    anchor.insertAdjacentHTML("afterend", html);
  }).catch(function (e) { console.warn("agents 注册表拉取失败,维持静态导航", e); });
}
injectNavAgents();
refreshAll().then(render).catch(function (e) { console.error(e); render(); });

/* 预设层搜索框绑定(input 事件,委托外单独绑) */
document.getElementById("presetSearch").addEventListener("input", function (e) {
  presetQuery = e.target.value || "";
  renderPresetGrid();
});
