// Captures README screenshots from the running dev app over the WebView2 DevTools protocol.
//
// 1. Start the app with a debugging port (PowerShell):
//      $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9333"; pnpm tauri dev
// 2. Optionally build the synthetic demo volume (no administrator rights needed):
//      cargo run -p unlost-core --example demo_image -- target/demo.img
// 3. Run: node scripts/screenshots.mjs [--out docs/screenshots] [--image target/demo.img]
//
// It drives the real app, switches theme, width and language, and saves PNGs. No browser is
// launched, so there is nothing to clean up afterwards.

import { mkdir, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";

const args = process.argv.slice(2);
const opt = (name, fallback) => (args.includes(name) ? args[args.indexOf(name) + 1] : fallback);
const outDir = opt("--out", "docs/screenshots");
const image = args.includes("--image") ? resolve(opt("--image")) : null;
const port = process.env.CDP_PORT ?? "9333";
const pages = opt("--pages", "start,recycle,versions,deleted,elsewhere,settings").split(",");
const themes = opt("--themes", "light,dark").split(",");
const widths = opt("--widths", "1440").split(",").map(Number);
const lang = opt("--lang", null);
const source = opt("--source", null);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function target() {
  for (let i = 0; i < 120; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const page = list.find((t) => t.type === "page" && /localhost:1430|tauri\.localhost/.test(t.url));
      if (page) return page;
    } catch {
      // The app is still starting.
    }
    await sleep(1000);
  }
  throw new Error(`no app page on port ${port}`);
}

function connect(wsUrl) {
  const ws = new WebSocket(wsUrl);
  let id = 0;
  const pending = new Map();
  ws.addEventListener("message", (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.id && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) reject(new Error(msg.error.message));
      else resolve(msg.result);
    }
  });
  const ready = new Promise((r) => ws.addEventListener("open", r));
  const send = async (method, params = {}) => {
    await ready;
    const n = ++id;
    ws.send(JSON.stringify({ id: n, method, params }));
    return new Promise((resolve, reject) => pending.set(n, { resolve, reject }));
  };
  return { send, close: () => ws.close() };
}

const t = await target();
const cdp = connect(t.webSocketDebuggerUrl);
const evaluate = async (expression) => {
  const r = await cdp.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.text + " " + (r.exceptionDetails.exception?.description ?? ""));
  return r.result.value;
};

await mkdir(outDir, { recursive: true });
await evaluate(`(async () => { for (let i = 0; i < 50 && !(window.__unlost && window.__unlost.getState().ready); i++) await new Promise(r => setTimeout(r, 200)); return !!window.__unlost })()`);
if (lang) await evaluate(`window.__unlostSetLang(${JSON.stringify(lang)})`);
if (source) await evaluate(`window.__unlost.getState().setSource(${JSON.stringify(source)})`);
if (image) {
  await evaluate(`(async () => {
    const s = window.__unlost.getState();
    s.setScan(await window.__unlostApi.scanStart({ kind: "image", path: ${JSON.stringify(image)}, partition: null }));
    return true;
  })()`);
}

for (const width of widths) {
  await cdp.send("Emulation.setDeviceMetricsOverride", { width, height: 900, deviceScaleFactor: 1, mobile: false });
  for (const theme of themes) {
    await evaluate(`document.documentElement.setAttribute('data-theme', ${JSON.stringify(theme)})`);
    for (const page of pages) {
      await evaluate(`window.__unlost.getState().go(${JSON.stringify(page)}); document.getElementById('main')?.scrollTo(0, 0)`);
      await sleep(1000);
      const shot = await cdp.send("Page.captureScreenshot", { format: "png" });
      const file = join(outDir, `${page}-${theme}${widths.length > 1 ? `-${width}` : ""}${lang ? `-${lang}` : ""}.png`);
      await writeFile(file, Buffer.from(shot.data, "base64"));
      console.log("saved", file);
    }
  }
}
await cdp.send("Emulation.clearDeviceMetricsOverride");
// Put the user's own theme and language back.
await evaluate(`(() => {
  const s = window.__unlost.getState().settings;
  const root = document.documentElement;
  if (s.theme === "light" || s.theme === "dark") root.setAttribute("data-theme", s.theme); else root.removeAttribute("data-theme");
  window.__unlostSetLang(s.language);
  window.__unlost.getState().go("start");
})()`);
cdp.close();
