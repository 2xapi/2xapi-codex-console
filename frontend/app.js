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
  sessions: null,       // 历史会话列表(GET /api/sessions)
  sessionsTotal: 0,
  sessionsPage: 1,
  sessionsProvider: "",
  sessionsRepairing: false,
  sessionsSettings: null, // {autoRepairBeforeHost}
  accel: null,            // GET /api/accel/state {mode, customNode, lines, scopeNote}
  nodeInput: null,        // 本机设置「我的加速节点」输入框草稿(重绘防丢)
  accelBusy: null,        // "mode" | "node-save" | "node-test"
  accelTest: null,        // 节点连通测试:{busy:true} | {ok:true,latencyMs} | {ok:false,msg}
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
/* bytes → GB 两位小数(官方线路配额展示专用) */
function fmtGb(bytes) { return (Number(bytes || 0) / 1073741824).toFixed(2); }
function fmtQuotaTotalGb(bytes) { return String(Math.round(Number(bytes || 10737418240) / 1073741824)); }
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
async function refreshAccel() {
  try { state.accel = await api.accelState(); } catch (e) { state.accel = state.accel || { mode: "off", customNode: "", lines: [], scopeNote: "", usage: { ok: false, degradedToDirect: false } }; }
}
async function refreshSession() {
  try { state.session = await api.session(); } catch (e) { state.session = null; }
  // 实时余额(auth/me;未登录/失败静默——顶栏显示 …)
  state.balance = null;
  if (state.session && (state.session.loggedIn || state.session.authenticated)) {
    try {
      var me = await api.me();
      var u = (me && me.user) || {};
      if (typeof u.balance === "number") state.balance = u.balance;
    } catch (e) { /* 下次刷新再试 */ }
  }
}
async function refreshAll() {
  await Promise.all([refreshProviders(), refreshDesktop(), refreshSession(), refreshAccel()]);
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
  // /api/session 形态:{authenticated, user:{email,...}}(兼容旧 loggedIn/email 顶层字段)
  var logged = !!(s && (s.authenticated || s.loggedIn || s.email || (s.user && s.user.email)));
  if (!logged) return '<button class="btn ghost" data-a="login">登录 2xapi</button>';
  var dispEmail = (s.user && s.user.email) || s.email || "已登录";
  // 余额 chip(实时,拉不到显示 …;低额警示色)
  var bal = state.balance;
  var balChip;
  if (bal == null) {
    balChip = '<span class="gw-chip">$…</span>';
  } else {
    var low = bal < 1;
    balChip = '<span class="gw-chip" style="' + (low ? "border-color:var(--c-err);color:var(--c-err)" : "") + '" title="2xapi 账号余额">' + (bal < 0 ? "-" : "") + "$" + Math.abs(bal).toFixed(2) + "</span>";
  }
  return balChip
    + '<button class="btn ghost" data-a="import">⇩ 导入 Key</button>'
    + '<button class="btn ghost" data-a="logout">' + esc(dispEmail) + " · 登出</button>";
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

/* 测试连接结果渲染(state.test: null | {busy:true} | {ok:true, data} | {ok:false, msg, field}) */
function testStepsHtml() {
  var t = state.test;
  var step = function (icon, text, meta, bad) {
    return '<div class="step' + (bad ? " bad" : "") + '">' + icon + " " + text + (meta ? '<span class="meta">' + esc(meta) + "</span>" : "") + "</div>";
  };
  if (t.busy) {
    return '<div id="rtest"><div class="steps" style="margin-top:12px">' + step("⟳", "测试连接进行中…", "密钥/协议/建议") + "</div></div>";
  }
  if (!t.ok) {
    // 失败:人话提示 + 高亮来源(连接=地址字段,认证=Key 字段)
    return '<div id="rtest"><div class="steps" style="margin-top:12px">'
      + step("✗", t.msg || "测试连接失败", t.meta, true)
      + "</div></div>";
  }
  var d = t.data;
  var steps = [];
  steps.push(step(d.keyOk ? "✓" : "✗", d.keyOk ? "密钥有效" : "密钥无效", (d.keyOk ? (d.models.length + " 个模型") : "") + " · " + d.latencyMs + "ms", !d.keyOk));
  var proto = d.responsesCompat ? "Responses 兼容" : (d.chatOk ? "仅 Chat(网关自动转换)" : "协议未测出");
  steps.push(step((d.responsesCompat || d.chatOk) ? "✓" : "✗", "协议判定:" + proto, d.responsesCompat ? "免转换" : (d.chatOk ? "需经网关转换" : ""), !(d.responsesCompat || d.chatOk)));
  if (d.suggest === "gateway") {
    steps.push(step("⚡", "建议方式:网关(推荐,零落盘)", "可一键开启托管"));
  } else if (d.error) {
    steps.push(step("✗", "无可用接入方式", d.error, true));
  } else {
    steps.push(step("⚡", "建议方式:网关", ""));
  }
  return '<div id="rtest"><div class="steps" style="margin-top:12px">' + steps.join("") + "</div></div>";
}

async function doTestConnection() {
  // 测当前选中供应商(未托管时主卡下拉即选中者;托管中测托管者)
  var pid = (hosting() && hosting().providerId) || state.selId;
  if (!pid) { showToast("请先选择或新建一个供应商", "error"); return; }
  state.test = { busy: true }; render();
  try {
    var d = await api.preflight({ providerId: pid });
    state.test = { ok: true, data: d };
  } catch (e) {
    state.test = { ok: false, msg: e.message, meta: "请求失败" };
  }
  render();
}

/* ── 桌面版主卡:账号状态自动检测 × 通路(网关 + 加速三态:关/官方线路/我的节点,阶段 4) ── */
function desktopCard() {
  var d = state.dstate || {};
  var hasOff = !!d.hasOfficial;
  var h = hosting();
  var isHost = !!(h && h.way === "gateway");
  var p = lineOf(state.selId) || lineOf(h && h.providerId);
  var modeName = hasOff ? "混入模式" : "纯 API 模式";
  var acctSub = hasOff ? "官方登录保留" : "纯 API · 无官方账号";

  var acc = state.accel || {};
  var accelMode = acc.mode || "off";                       // off | official | custom
  var accelLabel = accelMode === "official" ? "官方线路" : "我的节点";
  var accelOn = accelMode !== "off" && (accelMode === "official" || !!acc.customNode); // 是否走加速节点
  var usage = (acc.usage && acc.usage.ok) ? acc.usage : null; // 每账号凭证用量;未换取成功/缺省(ok:false)→ 不显示

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
      + (accelOn
        ? lk("var(--c-accel)", true) + st("var(--c-accel)", "加速节点", accelLabel) + lk("var(--c-accel)", true)
        : lk("var(--c-gw)", true))
      + st(p ? chipColor(p, state.providers.indexOf(p)) : "var(--c-gw)", esc(p ? p.name : "?"), "中转站");
    note = '<div class="route-mode"><span class="k">●</span>通路二:网关' + (accelOn ? " + 加速(" + accelLabel + ")" : "(加速已关,直发上游)") + ' · 配置文件零 Key,Key 由网关注入 · ' + modeName + '</div>'
      + (accelMode === "official" && acc.scopeNote ? '<div class="notice" style="margin:0 0 10px">' + esc(acc.scopeNote) + '</div>' : "")
      + (usage && usage.degradedToDirect
        ? '<div class="notice" style="margin:0 0 10px">⚠ 官方加速配额已用满,已自动切换直连;点「刷新凭证」重试或等待配额恢复</div>'
        : "");
    mech = (hasOff ? '<span>① 官方登录/插件保留</span>' : '<span>① 无需官方账号</span>')
      + '<span>② 配置文件零 Key</span><span>③ 协议转换 · chat 中转可用</span>'
      + '<span>④ 加速:' + (accelMode === "off" ? "关" : accelLabel) + '</span><span>依赖本 app 常驻</span>';
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
      ? '<div class="seg" style="margin-top:8px;max-width:460px">'
        + '<button data-a="accel" data-m="off" aria-pressed="' + (accelMode === "off") + '" style="--lc:var(--muted)">加速:关<small>网关直发上游</small></button>'
        + '<button data-a="accel" data-m="official" aria-pressed="' + (accelMode === "official") + '" style="--lc:var(--c-accel)">官方线路<small>2xapi 站专用 · 自动可用</small></button>'
        + '<button data-a="accel" data-m="custom" aria-pressed="' + (accelMode === "custom") + '" style="--lc:var(--c-accel)"' + (acc.customNode ? "" : " disabled") + '>我的节点<small>自己的 VPS / 本地代理</small></button>'
        + '</div>'
        + (accelMode === "official" && usage
          ? '<div class="hint" style="margin:6px 0 0' + (usage.quotaPercent >= 0.9 ? ";color:var(--c-err)" : "") + '">官方线路用量 ' + fmtGb(usage.quotaUsedBytes) + " G / " + fmtQuotaTotalGb(usage.quotaTotalBytes) + " G</div>"
          : "")
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
    + '<button class="btn ghost" data-a="test"' + (state.test && state.test.busy ? ' disabled' : '') + '>⚡ 测试连接</button>'
    + '</div>'
    + (state.test ? testStepsHtml() : '<div id="rtest"></div>')
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
    + kv("思考档位", (p.reasoning_levels || []).join(" / ") || "—", true)
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
  // 思考档位标签:探测到的档位;无则「—」
  var rl = (d.reasoning_levels || []).filter(function (x) { return x; });
  var rlBar = '<div class="rl-bar"><span class="k">思考档位</span>'
    + (rl.length ? rl.map(function (x) { return '<span class="rl-tag">' + esc(x) + '</span>'; }).join("")
      : '<span class="rl-muted">—</span>')
    + '</div>';
  var wireSel = d.wireApi === "chat_completions" ? "chat_completions" : "responses";
  // 推理强度下拉:自动(跟随探测) + 五档;回显草稿里选中的档位
  var rlCur = d.reasoningLevelSel || "";
  var rlOpts = [["", "自动(跟随探测)"], ["low", "low"], ["medium", "medium"], ["high", "high"], ["xhigh", "xhigh"], ["max", "max"]]
    .map(function (o) { return '<option value="' + o[0] + '"' + (rlCur === o[0] ? " selected" : "") + '>' + o[1] + '</option>'; }).join("");
  return '<section class="card"><h2>' + (state.isNew ? "新建供应商" : "编辑供应商 · " + esc(d.name)) + '</h2>'
    + '<div class="sub">填好地址和 Key,点「拉取模型」自动获取模型列表;Key 只存在本软件里,不写入任何配置文件。</div>'
    + '<div class="grid">'
    + '<div class="f full' + fc("name") + '"><label>名称 *</label><input data-f="name" value="' + esc(d.name || "") + '">' + fe("name") + '</div>'
    + '<div class="f full' + fc("baseUrl") + '"><label>上游地址 *</label><input class="mono" data-f="baseUrl" value="' + esc(d.baseUrl || "") + '" placeholder="https://api.example.com">' + fe("baseUrl") + '</div>'
    + '<div class="f full' + fc("apiKey") + '"><label>api key' + (state.isNew ? " *" : " · 留空不修改") + '</label><input type="password" class="mono" data-f="apiKey" placeholder="' + (state.isNew ? "sk-..." : (d.apiKeyMasked ? "•••• 未改则留空" : "sk-...")) + '" value="">' + fe("apiKey") + '</div>'
    + '<div class="f' + fc("model") + '"><label>默认模型 *</label><input class="mono" data-f="model" value="' + esc(d.model || "") + '" placeholder="点「拉取模型」后自动填入">' + fe("model") + '</div>'
    + "</div>"
    + '<div class="eyebrow" style="margin:16px 0 6px">模型列表(「拉取模型」自动填写,一般无需手改)</div>'
    + rlBar
    + '<table class="mtable"><thead><tr><th>模型名</th><th>上下文</th><th></th></tr></thead><tbody>' + rows + '</tbody></table>'
    + '<div class="btn-row">'
    + '<button class="btn ghost" data-a="mfetch"' + (state.busy === "mfetch" ? " disabled" : '') + '>' + (state.busy === "mfetch" ? "拉取中…" : "⤓ 拉取模型") + '</button>'
    + '<button class="btn ghost" data-a="mrow-add">＋ 手动加一行</button>'
    + '</div>'
    + '<details style="margin-top:10px"><summary>高级(协议 · 代理 · 超时 · 推理强度 · 备注)· 不用动</summary><div class="grid" style="margin-top:10px">'
    + '<div class="f"><label>协议</label><select data-f="wireSel"><option value="auto"' + (d.wireSelUi !== wireSel ? " selected" : "") + '>自动(拉取模型时检测)</option><option value="responses"' + (d.wireSelUi === "responses" ? " selected" : "") + '>Responses</option><option value="chat_completions"' + (d.wireSelUi === "chat_completions" ? " selected" : "") + '>ChatCompletions</option></select><div class="hint">不确定就保持「自动」</div></div>'
    + '<div class="f"><label>推理强度</label><select data-f="reasoningLevel">' + rlOpts + '</select><div class="hint">选具体档位 = 该供应商默认思考档位;「自动」= 跟随探测</div></div>'
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

/* ── 历史会话面板(阶段 3):列表/按供应商筛选/立刻修复/自动修复开关 ── */
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
function historyPane() {
  var s = state.sessions;
  var listHtml;
  if (state.sessionsRepairing) {
    listHtml = '<div class="sub">正在对账会话…(先整库备份,再核对会话文件)</div>';
  } else if (s === null) {
    listHtml = '<div class="sub">加载中…</div>';
  } else if (!s.length) {
    listHtml = '<div class="sub">没有会话记录' + (state.sessionsProvider ? "(已按供应商筛选)" : "") + "</div>";
  } else {
    var rows = s.map(function (it) {
      var tagColor = it.providerTag === "unknown" ? "" : 'style="border-color:var(--c-gw);color:var(--c-gw)"';
      return '<div class="run"><div class="what"><b>' + esc(it.title || "(无标题)") + '</b>'
        + '<span><span class="tag" ' + tagColor + '>' + esc(it.providerTag) + "</span>"
        + (it.missing ? ' <span class="tag" style="border-color:var(--c-err);color:var(--c-err)">会话文件缺失</span>' : "")
        + " · " + esc(fmtTime(it.updatedAt))
        + (it.cwd ? " · " + esc(it.cwd) : "") + "</span></div>"
        + '<button class="btn ghost" data-a="sess-continue" data-i="' + it.id + '">继续</button></div>';
    }).join("");
    listHtml = rows;
  }
  var providers = (state.providers || []).map(function (p) { return p.name; });
  providers = providers.concat(["custom", "2xapi"]);
  var unique = Array.from(new Set(providers));
  var filterOpts = '<option value="">全部供应商</option>' + unique.map(function (n) {
    return '<option value="' + esc(n) + '"' + (n === state.sessionsProvider ? " selected" : "") + ">" + esc(n) + "</option>";
  }).join("");
  var autoOn = !!(state.sessionsSettings && state.sessionsSettings.autoRepairBeforeHost);
  return '<div class="sub" style="margin-bottom:4px">Codex 对话记录(~/.codex 统一保存):共 <b>' + state.sessionsTotal + "</b> 条。</div>"
    + '<div style="display:flex;gap:8px;align-items:center;margin:0 0 10px;flex-wrap:wrap">'
    + '<select data-a="sess-filter" style="flex:1;min-width:140px;padding:7px 9px;background:var(--raised);border:1px solid var(--hair);border-radius:8px;color:var(--text);font-size:12.5px">' + filterOpts + "</select>"
    + '<button class="btn ghost" data-a="sess-repair"' + (state.sessionsRepairing ? " disabled" : "") + '>' + (state.sessionsRepairing ? "修复中…" : "立刻修复") + "</button>"
    + '<label style="display:flex;align-items:center;gap:6px;font-size:12px;color:var(--muted);cursor:pointer"><input type="checkbox" data-a="sess-autofix"' + (autoOn ? " checked" : "") + ">启动前自动修复</label>"
    + "</div>"
    + listHtml
    + (state.sessionsPage > 1 ? '<div class="btn-row" style="margin-top:8px"><button class="btn ghost" data-a="sess-prev">← 上一页</button></div>' : "");
}

async function openHistoryModal() {
  state.modal = { kind: "tool", t: "history" };
  render();
  await Promise.all([loadSessions(), api.sessionsSettings().then(function (d) { state.sessionsSettings = d; render(); }).catch(function () {})]);
}

async function loadSessions() {
  try {
    var d = await api.sessions(state.sessionsPage, 50, state.sessionsProvider);
    state.sessions = d.items || [];
    state.sessionsTotal = d.total || 0;
  } catch (e) {
    state.sessions = [];
    showToast("获取会话失败:" + e.message, "error");
  }
  render();
}

async function doSessionsRepair() {
  state.sessionsRepairing = true; render();
  try {
    var d = await api.sessionsRepair();
    showToast("修复完成:对账 " + d.scanned + " 条,修正 " + d.fixed + " 条(已先备份)", "ok");
  } catch (e) {
    showToast("修复失败:" + e.message, "error");
  }
  state.sessionsRepairing = false;
  await loadSessions();
}

/* ── 弹窗 ── */
function confirmHtml() {
  return '<div class="mask" style="z-index:70" data-a="cno"><div class="box" style="width:330px"><div style="margin-bottom:16px">' + esc(state.confirmBox.msg) + '</div><div class="btn-row" style="margin:0"><button class="btn danger" data-a="cyes">删除</button><button class="btn ghost" data-a="cno">取消</button></div></div></div>';
}
function modalHtml() {
  var m = state.modal;
  if (m.kind === "login") {
    return '<div class="mask" data-a="mclose"><div class="box" style="width:350px"><h2 style="margin:0 0 4px;font-size:15px">登录 2xapi 账号</h2>'
      + '<div class="sub">登录后可一键导入你的 Key 和供应商</div>'
      + (captchaCfg.enabled ? '<div class="hint" style="margin:0 0 6px;color:var(--c-direct)">该站点开启了登录验证,点「登录」后请完成滑块验证</div>' : "")
      + '<div class="f" style="margin:8px 0"><label>邮箱</label><input data-l="email" value="' + esc(state.loginEmail || "") + '"></div>'
      + '<div class="f" style="margin:8px 0"><label>密码</label><input type="password" data-l="password" value="' + esc(state.loginPassword || "") + '"></div>'
      + '<label style="display:flex;align-items:center;gap:8px;font-size:12.5px;color:var(--muted);cursor:pointer;margin:2px 0 8px"><input type="checkbox" data-l="remember" checked>记住我(保持登录,过期自动续期;滑块只需这一次)</label>'
      + (state.loginError ? '<div class="err" style="color:var(--c-err);font-size:12px">' + esc(state.loginError) + '</div>' : "")
      + '<div class="btn-row"><button class="btn primary" data-a="do-login">登录</button><button class="btn ghost" data-a="mclose">取消</button></div></div></div>';
  }
  if (m.kind === "snippet") {
    return '<div class="mask" data-a="mclose"><div class="box"><h2 style="margin:0 0 4px;font-size:15px">config 片段(进阶,可选)</h2>'
      + '<div class="sub">仅给想手动配置 ~/.codex 的用户:自行粘贴、自行负责。日常使用点「开启:桌面版走中转」即可,无需任何手动配置。</div>'
      + '<pre class="toml">model_provider = "custom"\n\n[model_providers.custom]\nname = "custom"\nbase_url = "http://127.0.0.1:8787"\nwire_api = "responses"\nrequires_openai_auth = true</pre>'
      + '<div class="btn-row"><button class="btn primary" data-a="copy-snippet">复制到剪贴板</button><button class="btn ghost" data-a="mclose">关闭</button></div></div></div>';
  }
  if (m.kind === "import") {
    var d = state.importData;
    var body;
    if (!d) {
      body = '<div class="sub">正在获取你的 Key 列表…</div>';
    } else if (!d.keys.length) {
      body = '<div class="sub">账号里还没有 Key,去 2xapi 网站创建后再来导入。</div>';
    } else {
      var rowsHtml = d.keys.map(function (k, i) {
        var keyStr = String(k.key || "");
        var masked = keyStr.length > 12 ? keyStr.slice(0, 6) + "…" + keyStr.slice(-4) : keyStr;
        var active = k.status === "active" || k.status === "enabled" || !k.status;
        var quota = (typeof k.quota === "number" && k.quota > 0)
          ? " · 额度 $" + k.quota.toFixed(2) + (k.quota_used ? "(已用 $" + Number(k.quota_used).toFixed(2) + ")" : "")
          : " · 不限量";
        return '<div class="run"><div class="what"><b>' + esc(k.name || ("Key " + (i + 1))) + '</b>'
          + '<span>' + esc(masked) + quota + (active ? "" : ' · <span style="color:var(--c-err)">' + esc(k.status) + "</span>") + "</span></div>"
          + '<button class="btn primary" data-a="import-key" data-i="' + i + '"' + (state.importBusy === i ? " disabled" : "") + '>' + (state.importBusy === i ? "导入中…" : "导入此 Key") + "</button></div>";
      }).join("");
      body = '<div class="sub" style="margin-bottom:8px">选择一个 Key,自动创建供应商(拉取模型列表、填好默认模型,开箱即用)。</div>' + rowsHtml;
    }
    return '<div class="mask" data-a="mclose"><div class="box" style="width:480px"><h2 style="margin:0 0 10px;font-size:15px">从 2xapi 账号导入 Key</h2>' + body
      + '<div class="btn-row" style="margin-top:12px"><button class="btn ghost" data-a="mclose">关闭</button></div></div></div>';
  }
  var body;
  if (m.t === "history") {
    body = historyPane();
  } else {
    var nodeVal = (state.nodeInput != null ? state.nodeInput : ((state.accel && state.accel.customNode) || ""));
    var at = state.accelTest;
    var testHtml;
    if (at && at.busy) testHtml = '<div class="hint" style="margin:6px 0 0">连通测试中…</div>';
    else if (at && at.ok) testHtml = '<div class="hint" style="margin:6px 0 0;color:var(--c-official)">✓ 连通 · 延迟 ' + at.latencyMs + 'ms</div>';
    else if (at && !at.ok) testHtml = '<div class="err" style="margin:6px 0 0">✗ ' + esc(at.msg) + '</div>';
    else testHtml = "";
    body = '<div class="kv"><div><div class="k">config.toml</div><div class="v mono">~/.codex(托管开启时仅一处 custom 段,零 Key)</div></div><div><div class="k">网关</div><div class="v mono">127.0.0.1:8787 · 托盘常驻</div></div></div>'
      + '<div class="f full" style="margin-top:12px"><label>我的加速节点(可选 · 仅本机保存,不上传)</label>'
      + '<div style="display:flex;gap:8px;align-items:flex-start">'
      + '<input class="mono" style="flex:1" data-anode value="' + esc(nodeVal) + '" placeholder="socks5://127.0.0.1:7890 或 http://用户:密码@你的VPS:8443">'
      + '<button class="btn ghost" data-a="node-save"' + (state.accelBusy === "node-save" ? " disabled" : "") + '>保存</button>'
      + '<button class="btn ghost" data-a="test-node"' + (state.accelBusy === "node-test" ? " disabled" : "") + '>测试节点连通</button>'
      + '</div>' + testHtml
      + '<div class="hint">自己的 VPS(跑 gost/squid)或本地代理客户端端口;填好后保存,主卡「加速」里即可选「我的节点」。官方加速专线由 2xapi 下发,无需填写。某个供应商若要固定走代理,用该供应商编辑里的「高级 · HTTP 代理」。</div></div>'
      + '<div class="f full" style="margin-top:14px"><label>官方加速凭证(每账号 10 G / 月)</label>'
      + '<div style="display:flex;gap:8px;align-items:center;flex-wrap:wrap">'
      + '<button class="btn ghost" data-a="node-refresh"' + (state.accelBusy === "node-refresh" ? " disabled" : "") + '>' + (state.accelBusy === "node-refresh" ? "刷新中…" : "刷新凭证") + '</button>'
      + '<span class="hint">重新换取本账号专属代理凭证并更新用量;配额用满会自动切直连,恢复后刷新即可重新加速。</span>'
      + '</div></div>';
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
  var rlv = get("reasoningLevel");
  if (rlv !== undefined) d.reasoningLevelSel = rlv; // "自动" = 跟随探测,落库 reasoning_levels 不变;显式选了才置首
  var mnames = document.querySelectorAll('[data-mf="name"]');
  mnames.forEach(function (el) { d.models[Number(el.dataset.mi)].name = el.value.trim(); });
  document.querySelectorAll('[data-mf="cw"]').forEach(function (el) {
    var v = el.value.trim();
    d.models[Number(el.dataset.mi)].contextWindow = v ? Number(v) : null;
  });
}

function draftFromProvider(p) {
  var rl = (p.reasoning_levels || []).slice();
  return {
    id: p.id, name: p.name, baseUrl: p.baseUrl || "", apiKey: "", apiKeyMasked: p.apiKeyMasked || "",
    wireApi: p.wireApi || "responses", wireSelUi: p.wireApi || "responses",
    model: p.model || "", models: (p.models || []).map(function (m) { return { name: m.name, contextWindow: m.contextWindow }; }),
    proxyUrl: p.proxyUrl || "", timeoutSecs: p.timeoutSecs || "", notes: p.notes || "",
    reasoning_levels: rl, reasoningLevelSel: rl.length ? rl[0] : "", // 编辑时下拉回显当前默认档位;无则「自动」
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

/* ── 加速(阶段 4):三态切换 / 我的节点保存 / 节点连通测试(契约:POST /api/accel/*,零耦合) ── */
async function doAccelMode(m) {
  var acc = state.accel || {};
  if (m === acc.mode) return;
  if (m === "custom" && !acc.customNode) { showToast("请先在本机设置里保存「我的加速节点」", "error"); return; }
  state.accelBusy = "mode"; render();
  try {
    await api.accelSetMode(m);
    state.accel.mode = m;
    showToast(m === "off" ? "加速已关闭,网关直发上游" : "加速已切换:" + (m === "official" ? "官方线路" : "我的节点"), "ok");
    if (m === "official") await doAccelRefreshCred(true); // 切官方线路:自动取一次最新用量(静默)
  } catch (e) {
    showToast(e.message, "error");
  }
  state.accelBusy = null; render();
}
/* 刷新官方加速凭证:重新换取本账号专属凭证并更新用量(silent=切换线路后自动取用量,不弹提示) */
async function doAccelRefreshCred(silent) {
  state.accelBusy = "node-refresh"; render();
  try {
    var r = await api.accelRefreshCred();
    if (!state.accel) state.accel = { mode: "off", customNode: "", lines: [], scopeNote: "", usage: { ok: false, degradedToDirect: false } };
    state.accel.usage = (r && r.usage) || { ok: false, degradedToDirect: false };
    var u = state.accel.usage;
    if (!silent) showToast(u && u.ok ? "已刷新(用量 " + fmtGb(u.quotaUsedBytes) + " G / " + fmtQuotaTotalGb(u.quotaTotalBytes) + "G)" : "已刷新", "ok");
  } catch (e) {
    if (!silent) showToast(e.message, "error");
  }
  state.accelBusy = null;
  await refreshAccel();
  render();
}
async function doAccelSaveNode() {
  var el = document.querySelector("[data-anode]");
  var endpoint = el ? el.value.trim() : "";
  if (!endpoint) { showToast("请先填写节点地址", "error"); return; }
  state.nodeInput = endpoint;
  state.accelBusy = "node-save"; render();
  try {
    await api.accelSetCustomNode(endpoint);
    state.accel.customNode = endpoint;
    showToast("我的加速节点已保存(仅本机)", "ok");
  } catch (e) {
    showToast(e.message, "error");
  }
  state.accelBusy = null; render();
}
async function doAccelTestNode() {
  var endpoint = (state.nodeInput != null ? state.nodeInput : ((state.accel && state.accel.customNode) || "")).trim();
  if (!endpoint) { showToast("请先填写节点地址", "error"); return; }
  state.accelTest = { busy: true }; render();
  try {
    var d = await api.accelTestNode(endpoint);
    state.accelTest = { ok: true, latencyMs: d.latencyMs };
  } catch (e) {
    state.accelTest = { ok: false, msg: e.message };
  }
  render();
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
    // 推理强度:选了具体档位 → 置为默认(数组首项,其余探测档位保留);「自动」→ 原样带回探测档位
    reasoning_levels: (function () {
      var rl = (d.reasoning_levels || []).filter(function (x) { return x && typeof x === "string"; });
      var sel = d.reasoningLevelSel || "";
      return sel ? [sel].concat(rl.filter(function (x) { return x !== sel; })) : rl;
    })(),
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
    // 接住探测到的思考档位(后端缺省 [];仅拉模型写回,不动用户已选的推理强度)
    d.reasoning_levels = Array.isArray(r.reasoning_levels) ? r.reasoning_levels.slice() : [];
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

/* ── 一键导入 2xapi Key(登录后):拉 Key 列表 → 选一条 → 拉模型 → 建供应商 ── */
async function openImportModal() {
  state.modal = { kind: "import" };
  state.importData = null;
  state.importBusy = -1;
  render();
  try {
    var d = await api.apiKeys();
    state.importData = { keys: (d && d.keys) || [], baseUrl: (d && d.baseUrl) || "" };
  } catch (e) {
    state.modal = null;
    showToast("获取 Key 列表失败:" + e.message + (String(e.message).indexOf("登录") >= 0 ? ",请先登录" : ""), "error");
  }
  render();
}

async function doImportKey(i) {
  var d = state.importData;
  if (!d || !d.keys[i]) return;
  var k = d.keys[i];
  state.importBusy = i; render();
  try {
    // 拉模型定默认模型(导入的供应商开箱即用)
    var fm = await api.fetchModels({ baseUrl: d.baseUrl, apiKey: k.key });
    var models = (fm.models || []).map(normModel);
    if (!models.length) { showToast("该 Key 拉不到模型列表,请先手动新建供应商填入", "error"); state.importBusy = -1; render(); return; }
    var name = (k.name && String(k.name).trim()) || ("2xapi-" + String(k.key || "").slice(-6));
    if (state.providers.some(function (p) { return p.name === name; })) name += " 2";
    var saved = await api.createProvider({
      name: name, accessMode: "pure_api",
      baseUrl: d.baseUrl, apiKey: k.key, wireApi: "responses",
      model: models[0].name, models: models,
      reasoning_levels: Array.isArray(fm.reasoning_levels) ? fm.reasoning_levels : [],
    });
    await refreshProviders();
    state.selId = saved.id;
    state.modal = null;
    showToast("已导入供应商「" + name + "」· " + models.length + " 个模型,默认 " + models[0].name, "ok");
  } catch (e) {
    showToast("导入失败:" + e.message, "error");
  }
  state.importBusy = -1; render();
}

/* ── 2xapi 登录(含腾讯滑块:站点开启验证码时弹出,人工完成后携带票据登录) ── */
var captchaCfg = { enabled: false, appId: "", loaded: false };

function loadTcaptchaJs(cb) {
  if (window.TencentCaptcha || captchaCfg.loaded) return cb();
  captchaCfg.loaded = true; // 防重复加载
  var s = document.createElement("script");
  s.src = "https://turing.captcha.qcloud.com/TCaptcha.js";
  s.onload = function () { cb(); };
  s.onerror = function () { captchaCfg.loaded = false; state.loginError = "验证码组件加载失败,请检查网络"; render(); };
  document.head.appendChild(s);
}

async function openLoginModal() {
  state.loginError = "";
  state.modal = { kind: "login" };
  render();
  // 预填记住的凭据(有则直接可点登录)
  try {
    var r = await api.remembered();
    if (r && r.remembered) {
      state.loginEmail = r.email || "";
      state.loginPassword = r.password || "";
      render();
    }
  } catch (e) { /* 无记住凭据 */ }
  // 查验证码开关(后端按主站→中转站顺序探测;拿到即缓存)
  try {
    var c = await api.captchaSettings();
    captchaCfg.enabled = !!(c && c.enabled);
    captchaCfg.appId = (c && String(c.appId || "")) || "";
    if (captchaCfg.enabled) {
      loadTcaptchaJs(function () { /* 预加载,点登录时秒弹 */ });
      render(); // 提示语:「登录需完成滑块验证」
    }
  } catch (e) { /* 查不到按无验证码处理,登录失败会显示服务端信息 */ }
}

async function doLogin() {
  var email = (document.querySelector('[data-l="email"]') || {}).value || "";
  var password = (document.querySelector('[data-l="password"]') || {}).value || "";
  if (!email || !password) { state.loginError = "邮箱和密码都要填"; render(); return; }

  var submit = async function (ticket, randstr) {
    try {
      await api.login(email.trim(), password, ticket, randstr);
      // 记住我(默认勾选):保存凭据;session 过期由后端 refresh_token 免滑块自动续期
      var remember = (document.querySelector('[data-l="remember"]') || {}).checked !== false;
      try { remember ? await api.remember(email.trim(), password) : await api.forget(); } catch (e) { /* 记住失败不影响登录 */ }
      state.modal = null; state.loginError = "";
      await refreshSession();
      showToast("登录成功" + (remember ? "(已记住,下次自动保持)" : ""), "ok");
    } catch (e) {
      state.loginError = e.message; render();
    }
  };

  if (captchaCfg.enabled && captchaCfg.appId) {
    // 站点开启腾讯验证码:弹出滑块,人工完成后携带 ticket/randstr 提交(Sub2API 字段)
    loadTcaptchaJs(function () {
      if (!window.TencentCaptcha) { state.loginError = "验证码组件未就绪,请重试"; render(); return; }
      var cap = new window.TencentCaptcha(captchaCfg.appId, function (res) {
        if (res && res.ret === 0) submit(res.ticket, res.randstr);
        // 用户关闭滑块:不提交,留在登录框
      });
      cap.show();
    });
  } else {
    submit("", "");
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
      state.draft = { name: "", baseUrl: "", apiKey: "", model: "", models: [], wireApi: "responses", wireSelUi: "responses", proxyUrl: "", timeoutSecs: "", notes: "", reasoning_levels: [], reasoningLevelSel: "" };
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
    case "login": openLoginModal(); break;
    case "import": openImportModal(); break;
    case "import-key": doImportKey(Number(t.dataset.i)); break;
    case "do-login": doLogin(); break;
    case "logout":
      api.logout().catch(function () {}).then(function () {
        return api.forget().catch(function () {}); // 登出同时清除记住的凭据
      }).then(function () {
        state.session = null; render(); showToast("已登出", "ok");
      }).catch(function (e) { showToast(e.message, "error"); });
      break;
    case "tool": if (t.dataset.t === "history") { openHistoryModal(); } else { state.modal = { kind: "tool", t: t.dataset.t }; render(); } break;
    case "sess-continue": showToast("继续历史会话:请在桌面版 Codex 里打开对应对话", "ok"); break;
    case "sess-repair": doSessionsRepair(); break;
    case "sess-prev": state.sessionsPage = Math.max(1, state.sessionsPage - 1); loadSessions(); break;
    case "mclose": state.modal = null; render(); break;
    case "cyes": { var c = state.confirmBox; state.confirmBox = null; render(); if (c) c.resolve(true); break; }
    case "cno": { var c2 = state.confirmBox; state.confirmBox = null; render(); if (c2) c2.resolve(false); break; }
    case "test": doTestConnection(); break;
    case "accel": doAccelMode(t.dataset.m); break;
    case "node-save": doAccelSaveNode(); break;
    case "node-refresh": doAccelRefreshCred(); break;
    case "test-node": doAccelTestNode(); break;
  }
});

/* 下拉/change:输入收集 + 供应商切换(已托管 = 热切换) */
document.addEventListener("change", function (ev) {
  var nd = ev.target.closest("[data-anode]");
  if (nd) { state.nodeInput = nd.value; return; }
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
  var sfilter = ev.target.closest("[data-a='sess-filter']");
  if (sfilter) {
    state.sessionsProvider = sfilter.value;
    state.sessionsPage = 1;
    loadSessions();
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
  if (ev.target.closest("[data-f], [data-mf], [data-l]")) collectDraft();
});
/* 输入中也收集(避免重绘时机丢半个字) */
document.addEventListener("input", function (ev) {
  var nd = ev.target.closest("[data-anode]");
  if (nd) { state.nodeInput = nd.value; return; }
  if (ev.target.closest("[data-f], [data-mf], [data-l]")) collectDraft();
});

/* ── 启动 ── */
refreshAll().then(render).catch(function (e) { console.error(e); render(); });
