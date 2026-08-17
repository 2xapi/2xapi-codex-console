// api-client.js — 2xapi Codex Console 前端 API 客户端（对齐 04 契约）
// 统一处理 {ok,data} / {ok:false,error:{code,message,fields}} 信封；/health 不走信封。
(function (global) {
  "use strict";

  async function request(method, path, { body } = {}) {
    const opts = {
      method,
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
    };
    if (body !== undefined) opts.body = JSON.stringify(body);
    let resp;
    try {
      resp = await fetch(path, opts);
    } catch (e) {
      const err = new Error("网络请求失败：" + (e && e.message ? e.message : e));
      err.code = "E_NETWORK";
      throw err;
    }
    const payload = await resp.json().catch(() => ({}));
    if (payload && payload.ok === true) return payload.data;
    // 错误信封兼容两种形态:04 契约 {error:{code,message,fields}};加速等路由 {error:"人话"(字符串)}
    const e = (payload && payload.error) || null;
    const err = new Error((typeof e === "string" ? e : (e && e.message)) || "请求失败 (" + resp.status + ")");
    err.code = (e && typeof e === "object" && e.code) || "E_UNKNOWN";
    err.fields = (e && e.fields) || null;
    err.status = resp.status;
    throw err;
  }

  // raw 请求（auth 子系统等不走 04 信封的路由）
  async function rawJson(method, path, body) {
    const opts = { method, headers: { "Content-Type": "application/json" }, credentials: "same-origin" };
    if (body !== undefined) opts.body = JSON.stringify(body);
    const resp = await fetch(path, opts);
    const payload = await resp.json().catch(() => ({}));
    if (!resp.ok) {
      const e = new Error((payload && (payload.error || payload.message)) || "请求失败 (" + resp.status + ")");
      e.status = resp.status;
      throw e;
    }
    return payload;
  }

  global.api = {
    // ── 供应商（04 §1）──
    listProviders: () => request("GET", "/api/providers"),
    createProvider: (p) => request("POST", "/api/providers", { body: p }),
    updateProvider: (id, p) => request("PUT", "/api/providers/" + encodeURIComponent(id), { body: p }),
    deleteProvider: (id) => request("DELETE", "/api/providers/" + encodeURIComponent(id)),
    reorderProviders: (ids) => request("PUT", "/api/providers/reorder", { body: { ids } }),
    activeProvider: () => request("GET", "/api/providers/active"),
    activate: (id) => request("POST", "/api/providers/activate", { body: { id } }),
    activateOfficial: () => request("POST", "/api/providers/activate-official"),
    previewConfig: (provider) => request("POST", "/api/providers/preview-config", { body: provider }),
    diagnose: (id) => request("POST", "/api/providers/diagnose", { body: { id } }),
    // 超融合 A 线一期:能力探测/标签/注册表
    probeCapabilities: (id, body) => request("POST", "/api/providers/" + encodeURIComponent(id) + "/probe-capabilities", { body }),
    capabilityTags: () => request("GET", "/api/capability-tags"),
    capabilitySet: (body) => request("POST", "/api/capability-tags", { body }),
    fetchModels: (body) => request("POST", "/api/providers/fetch-models", { body }),
    fetchBalance: (id) => request("POST", "/api/providers/fetch-balance", { body: { id } }),
    // ── 健康（不走信封，04 §2）──
    health: async () => (await fetch("/health")).json(),

    // ── 2xapi 登录子系统（契约外，key 获取入口；这些路由是 raw 响应，不走 04 信封）──
    session: async () => rawJson("GET", "/api/session"),
    // 用系统浏览器打开外链(官网);CSP 下 window.open 不走系统浏览器,经后端 spawn
    openUrl: (url) => request("POST", "/api/open-url", { body: { url } }),
    captchaSettings: async () => rawJson("GET", "/api/auth/captcha"),
    login: async (email, password, captchaTicket, captchaRandstr) =>
      rawJson("POST", "/api/auth/login", {
        email, password,
        captchaTicket: captchaTicket || "",
        captchaRandstr: captchaRandstr || "",
      }),
    logout: async () => rawJson("POST", "/api/auth/logout", {}),
    remembered: async () => rawJson("GET", "/api/auth/remembered"),
    remember: async (email, password) => rawJson("POST", "/api/auth/remember", { email, password }),
    forget: async () => rawJson("POST", "/api/auth/forget", {}),
    apiKeys: async () => rawJson("GET", "/api/auth/api-keys"),
    me: async () => rawJson("GET", "/api/auth/me"),
    keyGroups: async () => rawJson("GET", "/api/key-groups"),

    // ── Codex 启动器（M7，直连版）──
    launcherStart: (body) => request("POST", "/api/launcher/start", { body }),
    launcherStop: (sessionId) => request("POST", "/api/launcher/stop", { body: { sessionId } }),
    launcherStatus: () => request("GET", "/api/launcher/status"),

    // ── 桌面版托管开关（阶段 1，任务书 §1.1）──
    // host/unhost 的错误形态为 {"error": code, "message": msg}（非 04 信封），需单独剥出 code
    desktopState: () => request("GET", "/api/desktop/state"),
    // ── 开机自启(竞品吸收 1.1-3):launchd plist,{enabled} ──
    autostart: () => request("GET", "/api/autostart"),
    setAutostart: (enabled) => request("POST", "/api/autostart", { body: { enabled } }),
  // 多平台注册表(A 阶段,D3 导航数据源):{agents:[{id,name,tip,available,egress,hosting}]}
  agents: () => request("GET", "/api/desktop/agents"),
  // ── 生态管理(开发组·生态中心 A 段):MCP 服务器列表/操作 + 预设市场 ──
  ecoList: (agent) => request("GET", "/api/desktop/" + agent + "/eco"),
  ecoOp: (agent, body) => request("POST", "/api/desktop/" + agent + "/eco", { body }),
  ecoPresets: () => request("GET", "/api/desktop/eco-presets"),
  // ── 插件与能力市场(多模态引擎部 二期):raw_json 形态(非 04 信封,成功直接 {sources,…}/{plugins,…}/{ok:true}) ──
  plugList: () => rawJson("GET", "/api/plugins"),
  plugMarket: () => rawJson("GET", "/api/plugin-market"),
  plugInstall: (sourceId, pluginId) => rawJson("POST", "/api/plugin-market/install", { sourceId, pluginId }),
  plugToggle: (id, enabled) => rawJson("POST", "/api/plugins/" + encodeURIComponent(id) + "/toggle", { enabled }),
  plugRemove: (id) => rawJson("DELETE", "/api/plugins/" + encodeURIComponent(id)),
  plugSrcAdd: (id, name, url) => rawJson("POST", "/api/plugin-market/sources", { id, name, url }),
  plugSrcDel: (id) => rawJson("DELETE", "/api/plugin-market/sources/" + encodeURIComponent(id)),
  plugSrcList: (id) => rawJson("GET", "/api/plugin-market/sources/" + encodeURIComponent(id) + "/plugins"),
  // ── 插件市场 v3(插件市场开发文档 §四):详情/本地添加/配置/启停/更新/安装 ──
  plugDetail: (id) => rawJson("GET", "/api/plugins/" + encodeURIComponent(id)),
  plugLocal: (manifest) => rawJson("POST", "/api/plugins/local", manifest),
  plugConfig: (id, body) => rawJson("PUT", "/api/plugins/" + encodeURIComponent(id) + "/config", body),
  plugUpdate: (id) => rawJson("POST", "/api/plugins/" + encodeURIComponent(id) + "/update"),
  plugInstallId: (id) => rawJson("POST", "/api/plugins/" + encodeURIComponent(id) + "/install"),
    desktopHost: async (providerId, way) => {
      const resp = await fetch("/api/desktop/host", {
        method: "POST", headers: { "Content-Type": "application/json" }, credentials: "same-origin",
        body: JSON.stringify({ providerId, way }),
      });
      const payload = await resp.json().catch(() => ({}));
      if (resp.ok && payload && payload.ok === true) return payload.data;
      const err = new Error((payload && payload.message) || "托管失败 (" + resp.status + ")");
      err.code = (payload && payload.error) || "E_UNKNOWN";
      err.status = resp.status;
      throw err;
    },
    desktopUnhost: async () => {
      const resp = await fetch("/api/desktop/unhost", {
        method: "POST", headers: { "Content-Type": "application/json" }, credentials: "same-origin",
      });
      const payload = await resp.json().catch(() => ({}));
      if (resp.ok && payload && payload.ok === true) return payload.data;
      const err = new Error((payload && payload.message) || "还原失败 (" + resp.status + ")");
      err.code = (payload && payload.error) || "E_UNKNOWN";
      err.status = resp.status;
      throw err;
    },
    // 测试连接(阶段 2):{providerId} 或 {baseUrl, apiKey}
    preflight: (body) => request("POST", "/api/launcher/preflight", { body }),

    // ── 泛化 agent 路由(A 阶段 /api/desktop/:agent/*;hermes 等新平台使用,codex/claude 仍走具名)──
    // 错误形态同 desktopHost:{"error": code, "message": msg}
    agentState: (agent) => request("GET", "/api/desktop/" + agent + "/state"),
    agentHost: async (agent, providerId, way) => {
      const resp = await fetch("/api/desktop/" + agent + "/host", {
        method: "POST", headers: { "Content-Type": "application/json" }, credentials: "same-origin",
        body: JSON.stringify({ providerId: providerId, way: way || "gateway" }),
      });
      const payload = await resp.json().catch(() => ({}));
      if (resp.ok && payload && payload.ok === true) return payload.data;
      const err = new Error((payload && payload.message) || "托管失败 (" + resp.status + ")");
      err.code = (payload && payload.error) || "E_UNKNOWN";
      err.status = resp.status;
      throw err;
    },
    agentUnhost: async (agent) => {
      const resp = await fetch("/api/desktop/" + agent + "/unhost", {
        method: "POST", headers: { "Content-Type": "application/json" }, body: "{}",
      });
      const v = await resp.json().catch(() => null);
      if (!resp.ok || (v && v.ok === false)) throw new Error((v && v.error && v.error.message) || ("HTTP " + resp.status));
      return v && v.data !== undefined ? v.data : v;
    },
    // 注入式启动命令(gemini 等;返回 {command, env, hint...},Key 为占位)
    agentStart: async (agent, way, providerId) => {
      const resp = await fetch("/api/desktop/" + agent + "/start", {
        method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ way: way || "gateway", providerId: providerId || "" }),
      });
      const v = await resp.json().catch(() => null);
      if (!resp.ok || (v && v.ok === false)) throw new Error((v && v.error && v.error.message) || ("HTTP " + resp.status));
      return v && v.data !== undefined ? v.data : v;
    },

    // ── Claude 注入式托管(Claude 批次:后端返回注入信息,前端展示/复制;停用=前端本地态)──
    // claude-start 契约:成功 {ok:true, command, env:{ANTHROPIC_BASE_URL,ANTHROPIC_AUTH_TOKEN}, way, providerId, providerName, model}
    //   —— 字段在顶层(不走 data 信封);失败 {ok:false, error:{code,message}}(4xx)。Key 只在响应,不落盘。
    claudeStart: async (way, providerId) => {
      const resp = await fetch("/api/desktop/claude-start", {
        method: "POST", headers: { "Content-Type": "application/json" }, credentials: "same-origin",
        body: JSON.stringify({ way: way || "", providerId: providerId || "" }),
      });
      const payload = await resp.json().catch(() => ({}));
      if (payload && payload.ok === true) {
        return {
          command: payload.command || "",
          env: payload.env || {},
          way: payload.way || "",
          providerId: payload.providerId || "",
          providerName: payload.providerName || "",
          model: payload.model || "",
        };
      }
      const e = payload && payload.error;
      const err = new Error((e && e.message) || "Claude 注入失败(" + resp.status + ")");
      err.code = (e && e.code) || "E_UNKNOWN";
      err.status = resp.status;
      throw err;
    },
    // 后端无 claude-stop 接口(注入式无常驻进程):停用 = 前端清除注入态,本地即刻完成
    claudeStop: async () => ({ restored: true }),

    // ── 加速(阶段 4,任务书 §…):mode off|official|custom;customNode 仅本机保存 ──
    // GET /api/accel/state 返回非信封 {mode,customNode,lines[],scopeNote}(字段在顶层),失败时可能 {ok:false,error};用 rawJson 解顶层字段
    accelState: async () => {
      const p = await rawJson("GET", "/api/accel/state");
      if (p && p.ok === false) {
        const e = new Error((p && p.error) || "获取加速状态失败");
        e.status = (p && p.status) || 0;
        throw e;
      }
      // usage:每账号凭证用量(契约新增顶层块;旧后端/未换取成功时缺省 → 兜底 {ok:false})
      return {
        mode: p.mode, customNode: p.customNode || "", lines: p.lines || [], scopeNote: p.scopeNote || "",
        usage: p.usage || { ok: false, degradedToDirect: false },
      };
    },
    // ── 用量仪表盘(用量仪表盘后端批次):GET /api/usage-stats 非信封 {providers:[{providerId,providerName,count,p50Ms,p90Ms,okRate,lastTs,routes}]} ──
    usageStats: async () => {
      const p = await rawJson("GET", "/api/usage-stats");
      return p && Array.isArray(p.providers) ? p.providers : [];
    },

    // refresh-cred 契约:200 {ok:true, usage:{...}} / 4xx {error:"人话"}(非信封,顶层字段;4xx 由 rawJson 抛 error)
    accelRefreshCred: async () => {
      const p = await rawJson("POST", "/api/accel/refresh-cred", {});
      if (!p || p.ok !== true) {
        const e = new Error((p && p.error) || "刷新凭证失败");
        e.status = (p && p.status) || 0;
        throw e;
      }
      return { ok: true, usage: p.usage || { ok: false, degradedToDirect: false } };
    },
    // mode/custom-node 契约走 04 信封 → {ok:true}
    accelSetMode: (mode) => request("POST", "/api/accel/mode", { body: { mode } }),
    accelSetCustomNode: (endpoint) => request("POST", "/api/accel/custom-node", { body: { endpoint } }),
    // test-node 契约是 {ok:true, latencyMs:123} / {ok:false, error:"人话"}——字段在顶层不走 data 信封,故用 rawJson
    accelTestNode: async (endpoint) => {
      const p = await rawJson("POST", "/api/accel/test-node", { endpoint });
      if (!p || p.ok !== true) { const e = new Error((p && p.error) || "测试节点失败"); e.status = (p && p.status) || 0; throw e; }
      return p; // {ok:true, latencyMs}
    },

    // ── 历史会话管理(阶段 3,任务书 §四)──
    sessions: (page, size, provider) => request("GET", "/api/sessions?page=" + (page || 1) + "&size=" + (size || 50) + "&provider=" + encodeURIComponent(provider || "")),
    sessionsRepair: () => request("POST", "/api/sessions/repair"),
    sessionsSettings: () => request("GET", "/api/sessions/settings"),
    sessionsSetSettings: (autoRepair) => request("POST", "/api/sessions/settings", { body: { autoRepairBeforeHost: autoRepair } }),
    // ── Claude 会话历史(R2:只读列表;~/.claude/projects jsonl,无修复/删除)──
    claudeSessions: (page, size) => request("GET", "/api/claude/sessions?page=" + (page || 1) + "&size=" + (size || 50)),

    // ── 运维：备份/快照/恢复/历史诊断（旧路由，raw 响应）──
    backups: async () => rawJson("GET", "/api/backups"),
    snapshot: async () => rawJson("POST", "/api/config/snapshot", {}),
    restoreConfig: async (backupPath) => rawJson("POST", "/api/config/restore", { backupPath }),
    inspectHistory: async () => rawJson("GET", "/api/history/inspect"),
  };
})(window);
