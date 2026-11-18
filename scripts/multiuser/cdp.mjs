// Minimal Chrome DevTools Protocol client for driving the running ASR Support Desk windows (WebView2).
// No dependencies: Node 22+ has fetch and WebSocket built in.
//
// Each test instance is started with ASR_TEST_MODE=1 and ASR_DEVTOOLS_PORT (see launch_multiuser_test.ps1),
// so its WebView2 listens on 127.0.0.1:<port>. `invoke` calls the app's real Tauri commands in that window
// (exactly what the UI buttons call); `ui` helpers click and read the real rendered page.

export class Instance {
  constructor(name, port) {
    this.name = name;
    this.port = port;
    this.id = 0;
    this.pending = new Map();
  }

  async connect(timeoutMs = 60000) {
    const until = Date.now() + timeoutMs;
    let target;
    while (!target) {
      try {
        const list = await (await fetch(`http://127.0.0.1:${this.port}/json/list`)).json();
        target = list.find((t) => t.type === "page" && /tauri\.localhost|localhost:1420/.test(t.url));
      } catch {
        /* not up yet */
      }
      if (!target) {
        if (Date.now() > until) throw new Error(`${this.name}: no DevTools page on port ${this.port}`);
        await sleep(500);
      }
    }
    this.ws = new WebSocket(target.webSocketDebuggerUrl);
    await new Promise((res, rej) => {
      this.ws.onopen = res;
      this.ws.onerror = (e) => rej(new Error(`${this.name}: websocket error ${e.message ?? ""}`));
    });
    this.ws.onmessage = (ev) => {
      const m = JSON.parse(ev.data);
      if (m.id && this.pending.has(m.id)) {
        const { resolve, reject } = this.pending.get(m.id);
        this.pending.delete(m.id);
        if (m.error) reject(new Error(`${this.name}: ${m.error.message}`));
        else resolve(m.result);
      }
    };
    this.ws.onclose = () => {
      for (const { reject } of this.pending.values()) reject(new Error(`${this.name}: connection closed`));
      this.pending.clear();
    };
    await this.send("Runtime.enable");
    // dialogs would block the page: answer them automatically (recorded for the report)
    await this.eval(`(() => { window.__asrDialogs = []; window.confirm = (m) => { window.__asrDialogs.push(['confirm', m]); return true; };
      window.prompt = (m, d) => { window.__asrDialogs.push(['prompt', m]); return window.__asrPromptAnswer ?? d ?? 'Automated test'; };
      window.alert = (m) => { window.__asrDialogs.push(['alert', m]); }; return true; })()`);
    // wait for the app to be ready (Tauri internals + rendered shell)
    await this.waitFor(`!!window.__TAURI_INTERNALS__ && !!document.querySelector('.shell nav')`, 60000);
    return this;
  }

  close() {
    try {
      this.ws?.close();
    } catch {
      /* ignore */
    }
  }

  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  /** Evaluate an expression in the page (promises awaited), return its JSON value. */
  async eval(expr, timeoutMs = 120000) {
    let timer;
    const r = await Promise.race([
      this.send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true }),
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${this.name}: evaluate timed out after ${timeoutMs} ms: ${expr.slice(0, 80)}`)), timeoutMs);
      }),
    ]).finally(() => clearTimeout(timer));
    if (r.exceptionDetails) throw new Error(`${this.name}: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
    return r.result.value;
  }

  /** Call a real Tauri command of this instance. Rejects with the command's error message. */
  async invoke(cmd, args = {}, timeoutMs = 180000) {
    const r = await this.eval(
      `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args)}).then((v) => ({ ok: true, v }), (e) => ({ ok: false, e: (e && e.message) || String(e), code: e && e.code }))`,
      timeoutMs,
    );
    if (!r.ok) {
      const err = new Error(`${this.name} ${cmd}: ${r.e}`);
      err.code = r.code;
      throw err;
    }
    return r.v;
  }

  /** invoke, but return {ok, v | e, code} instead of throwing */
  async tryInvoke(cmd, args = {}, timeoutMs = 180000) {
    try {
      return { ok: true, v: await this.invoke(cmd, args, timeoutMs) };
    } catch (e) {
      return { ok: false, e: e.message, code: e.code };
    }
  }

  async waitFor(expr, timeoutMs = 30000, everyMs = 250) {
    const until = Date.now() + timeoutMs;
    for (;;) {
      try {
        if (await this.eval(`(() => { try { return !!(${expr}); } catch { return false; } })()`, 10000)) return true;
      } catch {
        /* page reloading */
      }
      if (Date.now() > until) throw new Error(`${this.name}: timed out waiting for ${expr.slice(0, 100)}`);
      await sleep(everyMs);
    }
  }

  // ------------------------------------------------------------- UI helpers (real rendered page)

  /** Click the first button/link whose visible text matches. */
  async click(text, scope = "document") {
    const ok = await this.eval(`(() => {
      const want = ${JSON.stringify(text)};
      const els = [...${scope}.querySelectorAll('button, a, summary, label')].filter((b) => !b.disabled && b.offsetParent !== null);
      const el = els.find((b) => b.textContent.trim() === want) ?? els.find((b) => b.textContent.trim().startsWith(want));
      if (!el) return false;
      el.click();
      return true;
    })()`);
    if (!ok) throw new Error(`${this.name}: no visible button "${text}"`);
    await sleep(150);
  }

  async nav(label) {
    await this.click(label, "document.querySelector('.shell nav')");
    await sleep(300);
  }

  async text(selector = "main") {
    return this.eval(`(document.querySelector(${JSON.stringify(selector)})?.innerText ?? '')`);
  }

  async setInput(selector, value) {
    await this.eval(`(() => { const el = document.querySelector(${JSON.stringify(selector)}); if (!el) return false;
      const proto = el.tagName === 'SELECT' ? HTMLSelectElement.prototype : el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, ${JSON.stringify(value)});
      el.dispatchEvent(new Event('input', { bubbles: true })); el.dispatchEvent(new Event('change', { bubbles: true })); return true; })()`);
    await sleep(200);
  }

  /** Measure UI responsiveness: round trip of a trivial evaluation (ms). */
  async ping() {
    const t = Date.now();
    await this.eval("1+1", 30000);
    return Date.now() - t;
  }
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
