// LSP probe harness: same probes against any TS language server.
// usage: node lsp_probe.mjs <tsgo7|preview|tsls6> [rootDir]
import { spawn, execSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const NM = path.join(HERE, "node_modules");
const which = process.argv[2] ?? "tsgo7";
const ROOT = process.argv[3] ?? "/tmp/zodspike";
const SRC = path.join(ROOT, "packages/zod/src");

const SERVERS = {
  tsgo7: { cmd: [path.join(NM, "@typescript/typescript-linux-x64/lib/tsc"), "--lsp", "--stdio"] },
  preview: { cmd: [path.join(NM, "@typescript/native-preview-linux-x64/lib/tsgo"), "--lsp", "--stdio"] },
  tsls6: {
    cmd: [process.execPath, path.join(NM, "typescript-language-server/lib/cli.mjs"), "--stdio"],
    init: { tsserver: { path: path.join(NM, "ts-classic/lib/tsserver.js"), useSyntaxServer: process.env.SYNTAX ?? "auto" }, maxTsServerMemory: 8192 },
  },
};
const srv = SERVERS[which];
const proc = spawn(srv.cmd[0], srv.cmd.slice(1), { cwd: ROOT, stdio: ["pipe", "pipe", "pipe"] });
const stderrLog = fs.createWriteStream(path.join(HERE, `results/${which}.stderr.log`));
proc.stderr.pipe(stderrLog);

// ---------- JSON-RPC framing ----------
let buf = Buffer.alloc(0), nextId = 1;
const pending = new Map();
const notifications = [];
proc.stdout.on("data", (d) => {
  buf = Buffer.concat([buf, d]);
  for (;;) {
    const he = buf.indexOf("\r\n\r\n");
    if (he < 0) return;
    const m = /Content-Length: (\d+)/i.exec(buf.subarray(0, he).toString());
    const len = +m[1];
    if (buf.length < he + 4 + len) return;
    const msg = JSON.parse(buf.subarray(he + 4, he + 4 + len).toString("utf8"));
    buf = buf.subarray(he + 4 + len);
    if (msg.id !== undefined && msg.method) {
      // server->client request
      let result = null;
      if (msg.method === "workspace/configuration") result = msg.params.items.map(() => ({}));
      send({ jsonrpc: "2.0", id: msg.id, result });
    } else if (msg.id !== undefined) {
      const p = pending.get(msg.id); pending.delete(msg.id);
      msg.error ? p.rej(new Error(JSON.stringify(msg.error))) : p.res(msg.result);
    } else notifications.push(msg);
  }
});
function send(o) {
  const s = Buffer.from(JSON.stringify(o), "utf8");
  proc.stdin.write(`Content-Length: ${s.length}\r\n\r\n`); proc.stdin.write(s);
}
function req(method, params, timeoutMs = 300000) {
  const id = nextId++;
  send({ jsonrpc: "2.0", id, method, params });
  return new Promise((res, rej) => {
    pending.set(id, { res, rej });
    setTimeout(() => { if (pending.has(id)) { pending.delete(id); rej(new Error("timeout " + method)); } }, timeoutMs);
  });
}
const notify = (method, params) => send({ jsonrpc: "2.0", method, params });
async function timed(label, f) {
  const t = performance.now();
  try { const r = await f(); return { r, ms: +(performance.now() - t).toFixed(1) }; }
  catch (e) { return { err: String(e.message).slice(0, 300), ms: +(performance.now() - t).toFixed(1) }; }
}

// ---------- position helpers ----------
let ENC = "utf-16";
const uri = (f) => pathToFileURL(f).href;
const text = (f) => fs.readFileSync(f, "utf8");
function col(lineText, jsIdx) { // convert JS (utf-16) index to negotiated encoding
  if (ENC === "utf-16") return jsIdx;
  if (ENC === "utf-8") return Buffer.byteLength(lineText.slice(0, jsIdx), "utf8");
  return [...lineText.slice(0, jsIdx)].length;
}
// position of `needle` (nth occurrence) on 1-based line
function pos(file, line1, needle, nth = 0, off = 1) {
  const lt = text(file).split("\n")[line1 - 1];
  let i = -1; for (let k = 0; k <= nth; k++) i = lt.indexOf(needle, i + 1);
  if (i < 0) throw new Error(`needle ${needle} not on ${file}:${line1}`);
  return { textDocument: { uri: uri(file) }, position: { line: line1 - 1, character: col(lt, i + off) } };
}
function summarizeLocs(locs) {
  locs = locs ?? [];
  const files = new Set(locs.map((l) => l.uri));
  return { count: locs.length, files: files.size, probeHits: locs.filter((l) => l.uri.endsWith("alias_probe.ts")).map((l) => [l.range.start.line + 1, l.range.start.character]) };
}
function rssTree(pid) {
  const all = fs.readdirSync("/proc").filter((d) => /^\d+$/.test(d));
  const kids = (p) => all.filter((d) => { try { return fs.readFileSync(`/proc/${d}/stat`, "utf8").split(") ")[1].split(" ")[1] == p; } catch { return false; } });
  const tree = [String(pid)]; for (let i = 0; i < tree.length; i++) tree.push(...kids(tree[i]));
  let hwm = 0, rss = 0;
  for (const p of tree) { try { const s = fs.readFileSync(`/proc/${p}/status`, "utf8"); hwm += +/VmHWM:\s+(\d+)/.exec(s)[1]; rss += +/VmRSS:\s+(\d+)/.exec(s)[1]; } catch {} }
  return { procs: tree.length, peakMB: Math.round(hwm / 1024), rssMB: Math.round(rss / 1024) };
}

// ---------- probes ----------
const SCHEMAS = path.join(SRC, "v4/classic/schemas.ts");
const CPARSE = path.join(SRC, "v4/core/parse.ts");
const PROBE = path.join(SRC, "alias_probe.ts");
const out = { server: which, cmd: srv.cmd.join(" ") };
const T0 = performance.now();
const init = await timed("initialize", () => req("initialize", {
  processId: process.pid, rootUri: uri(ROOT), workspaceFolders: [{ uri: uri(ROOT), name: "zod" }],
  initializationOptions: srv.init,
  capabilities: {
    general: { positionEncodings: ["utf-8", "utf-16"] },
    textDocument: {
      hover: { contentFormat: ["plaintext", "markdown"] }, rename: { prepareSupport: true },
      callHierarchy: {}, references: {}, definition: { linkSupport: false }, synchronization: { didSave: true },
    },
    workspace: { workspaceEdit: { documentChanges: true }, configuration: true, workspaceFolders: true },
    window: { workDoneProgress: false },
  },
}));
const caps = init.r?.capabilities ?? {};
ENC = caps.positionEncoding ?? "utf-16";
out.initialize = { ms: init.ms, err: init.err, positionEncoding: caps.positionEncoding ?? "(absent => utf-16)",
  serverInfo: init.r?.serverInfo,
  caps: Object.keys(caps).filter((k) => k.endsWith("Provider")).sort() };
notify("initialized", {});
for (const f of [SCHEMAS, PROBE, ...(process.env.OPEN_ALL ? [CPARSE, path.join(SRC, "v4/core/util.ts")] : [])]) notify("textDocument/didOpen", { textDocument: { uri: uri(f), languageId: "typescript", version: 1, text: text(f) } });

if (process.env.WAIT_MS) { await new Promise((r) => setTimeout(r, +process.env.WAIT_MS)); out.waitedMs = +process.env.WAIT_MS; }
// 1. references on interface member ZodType.safeParse (schemas.ts:72)
const refIface = pos(SCHEMAS, 72, "safeParse");
let a = await timed("refs", () => req("textDocument/references", { ...refIface, context: { includeDeclaration: true } }));
out.refs_iface_cold = { ms: a.ms, err: a.err, ...summarizeLocs(a.r), sinceSpawnMs: +(performance.now() - T0).toFixed(0) };
a = await timed("refs", () => req("textDocument/references", { ...refIface, context: { includeDeclaration: true } }));
out.refs_iface_warm = { ms: a.ms, ...summarizeLocs(a.r) };
out.mem_after_refs = rssTree(proc.pid);
// 2. refs on implementation (object literal member inside ZodType init) schemas.ts:303
a = await timed("refs", () => req("textDocument/references", { ...pos(SCHEMAS, 303, "safeParse"), context: { includeDeclaration: true } }));
out.refs_impl = { ms: a.ms, err: a.err, ...summarizeLocs(a.r) };
// 3. refs on function `string` (re-exported via external.ts -> z namespace -> `import { z as zz }`)
a = await timed("refs", () => req("textDocument/references", { ...pos(SCHEMAS, 619, "string"), context: { includeDeclaration: true } }));
out.refs_string_fn = { ms: a.ms, err: a.err, ...summarizeLocs(a.r) };
// 4. refs on alias `zz`
a = await timed("refs", () => req("textDocument/references", { ...pos(PROBE, 1, "zz"), context: { includeDeclaration: true } }));
out.refs_alias_zz = { ms: a.ms, err: a.err, ...summarizeLocs(a.r) };
// 5. definition through alias: zz.string -> schemas.ts
a = await timed("def", () => req("textDocument/definition", pos(PROBE, 4, "string")));
out.definition_zz_string = { ms: a.ms, err: a.err, r: (Array.isArray(a.r) ? a.r : [a.r]).filter(Boolean).map((l) => `${path.relative(SRC, fileURLToPath(l.uri ?? l.targetUri))}:${(l.range ?? l.targetSelectionRange).start.line + 1}`) };
// 6. hover
for (const [k, ln, nd] of [["hover_o", 6, "o ="], ["hover_r2", 7, "r2"]]) {
  a = await timed("hover", () => req("textDocument/hover", pos(PROBE, ln, nd, 0, 0)));
  const v = a.r?.contents; out[k] = { ms: a.ms, err: a.err, text: (typeof v === "string" ? v : v?.value ?? JSON.stringify(v))?.slice(0, 220) };
}
// 7. call hierarchy (incoming) on `function object` and on core `safeParse` const
for (const [k, f, ln, nd] of [["calls_object_fn", SCHEMAS, 1731, "object"], ["calls_core_safeParse_const", CPARSE, 80, "safeParse"], ["calls_util_clone", path.join(SRC, "v4/core/util.ts"), 645, "clone"]]) {
  const p = await timed("prep", () => req("textDocument/prepareCallHierarchy", pos(f, ln, nd)));
  if (!p.r?.length) { out[k] = { err: p.err ?? "no item", ms: p.ms }; continue; }
  a = await timed("incoming", () => req("callHierarchy/incomingCalls", { item: p.r[0] }));
  out[k] = { prepMs: p.ms, ms: a.ms, err: a.err, item: p.r[0].name, callers: a.r?.length, callSites: a.r?.reduce((s, c) => s + c.fromRanges.length, 0), sample: a.r?.slice(0, 3).map((c) => c.from.name) };
}
// 8. prepareRename + rename across files
a = await timed("prepRename", () => req("textDocument/prepareRename", refIface));
out.prepareRename = { ms: a.ms, err: a.err, r: a.r };
a = await timed("rename", () => req("textDocument/rename", { ...refIface, newName: "safeParseZ" }));
if (a.r) {
  const edits = a.r.documentChanges ? a.r.documentChanges.flatMap((d) => d.edits.map((e) => [d.textDocument.uri, e])) : Object.entries(a.r.changes).flatMap(([u, es]) => es.map((e) => [u, e]));
  fs.writeFileSync(path.join(HERE, `results/${which}.rename.json`), JSON.stringify({ encoding: ENC, edit: a.r }));
  out.rename = { ms: a.ms, edits: edits.length, files: new Set(edits.map((e) => e[0])).size };
} else out.rename = { ms: a.ms, err: a.err };
// 9. UTF-8 / non-ASCII: references from `safeParse` after Cyrillic+CJK+emoji on line 7
a = await timed("refs", () => req("textDocument/references", { ...pos(PROBE, 7, "safeParse"), context: { includeDeclaration: false } }));
const l7 = text(PROBE).split("\n")[6]; const want = col(l7, l7.indexOf("safeParse"));
const got = (a.r ?? []).find((l) => l.uri.endsWith("alias_probe.ts") && l.range.start.line === 6);
out.nonascii = { enc: ENC, expectedChar: want, gotChar: got?.range.start.character, ok: got?.range.start.character === want, count: a.r?.length, err: a.err };
// 10. incremental: didChange adds one more safeParse call, re-query
const newText = text(PROBE) + "export const r3 = zz.boolean().safeParse(true);\n";
notify("textDocument/didChange", { textDocument: { uri: uri(PROBE), version: 2 }, contentChanges: [{ text: newText }] });
a = await timed("refs", () => req("textDocument/references", { ...refIface, context: { includeDeclaration: true } }));
out.after_didChange = { ms: a.ms, err: a.err, ...summarizeLocs(a.r), delta: (a.r?.length ?? 0) - out.refs_iface_warm.count };
a = await timed("hover", () => req("textDocument/hover", { textDocument: { uri: uri(PROBE) }, position: { line: 7, character: 14 } }));
out.after_didChange_hover_r3 = { ms: a.ms, text: (a.r?.contents?.value ?? JSON.stringify(a.r?.contents))?.slice(0, 160) };
out.mem_final = rssTree(proc.pid);

await timed("shutdown", () => req("shutdown", null, 5000)); notify("exit", null);
setTimeout(() => proc.kill("SIGKILL"), 1000);
fs.writeFileSync(path.join(HERE, `results/${which}${process.env.WAIT_MS ? ".wait" : ""}${process.env.SYNTAX ? "." + process.env.SYNTAX : ""}.json`), JSON.stringify(out, null, 2));
console.log(JSON.stringify(out, null, 2));
process.exit(0);
