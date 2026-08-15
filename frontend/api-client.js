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
    const e = (payload && payload.error) || {};
    const err = new Error(e.message || "请求失败 (" + resp.status + ")");
    err.code = e.code || "E_UNKNOWN";
    err.fields = e.fields || null;
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
    fetchModels: (body) => request("POST", "/api/providers/fetch-models", { body }),
    fetchBalance: (id) => request("POST", "/api/providers/fetch-balance", { body: { id } }),
    // ── 健康（不走信封，04 §2）──
    health: async () => (await fetch("/health")).json(),

    // ── 2xapi 登录子系统（契约外，key 获取入口；这些路由是 raw 响应，不走 04 信封）──
    session: async () => rawJson("GET", "/api/session"),
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

    // ── 运维：备份/快照/恢复/历史诊断（旧路由，raw 响应）──
    backups: async () => rawJson("GET", "/api/backups"),
    snapshot: async () => rawJson("POST", "/api/config/snapshot", {}),
    restoreConfig: async (backupPath) => rawJson("POST", "/api/config/restore", { backupPath }),
    inspectHistory: async () => rawJson("GET", "/api/history/inspect"),
  };
})(window);
