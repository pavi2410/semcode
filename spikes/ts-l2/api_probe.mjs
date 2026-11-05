// Probe TS7's programmatic API (typescript/unstable/sync): `tsc --api` speaking msgpack over stdio.
import { API } from "typescript/unstable/sync";
import fs from "node:fs";
const ROOT = process.argv[2] ?? "/tmp/zodspike";
const SRC = `${ROOT}/packages/zod/src`, SCHEMAS = `${SRC}/v4/classic/schemas.ts`, PROBE = `${SRC}/alias_probe.ts`;
const t = () => performance.now(); const out = {};
const offsetOf = (f, line1, needle) => { const L = fs.readFileSync(f, "utf8").split("\n"); let o = 0; for (let i = 0; i < line1 - 1; i++) o += L[i].length + 1; return o + L[line1 - 1].indexOf(needle); };
let s = t();
const api = new API({ cwd: ROOT, collectTiming: true });
let snap = api.updateSnapshot({ openProjects: [`${ROOT}/packages/zod/tsconfig.json`] });
const proj = snap.getProjects()[0];
out.load = { ms: +(t() - s).toFixed(0), project: proj.configFileName, files: proj.program.getSourceFileNames().length };
// symbol + type at position (offset = JS/UTF-16 string index?)
s = t(); const pOff = offsetOf(PROBE, 6, "o =");
const typ = proj.checker.getTypeAtPosition(PROBE, pOff);
out.typeAt_o = { ms: +(t() - s).toFixed(1), type: typ && proj.checker.typeToString(typ) };
// non-ASCII line: utf16 index vs utf8 byte offset of `safeParse` on line 7
const txt = fs.readFileSync(PROBE, "utf8"); const u16 = offsetOf(PROBE, 7, "safeParse"); const u8 = Buffer.byteLength(txt.slice(0, u16));
out.nonascii = { u16, u8, symAtU16: proj.checker.getSymbolAtPosition(PROBE, u16)?.name, symAtU8: proj.checker.getSymbolAtPosition(PROBE, u8)?.name };
// find-all-references for ZodType.safeParse
s = t(); const sf = proj.program.getSourceFile(SCHEMAS); out.getSourceFile_ms = +(t() - s).toFixed(1);
const pos = offsetOf(SCHEMAS, 72, "safeParse");
const findNode = (n) => { let hit; const visit = (c) => { if (c.pos <= pos && pos < c.end) { hit = c; c.forEachChild(visit); } }; visit(n); return hit; };
const node = findNode(sf);
out.nodeKind = node?.kind;
s = t(); const refs = proj.checker.getReferencedSymbolsForNode(node, pos);
out.refs = { ms: +(t() - s).toFixed(0), entries: refs.length, total: refs.reduce((a, e) => a + e.references.length, 0), files: new Set(refs.flatMap((e) => e.references.map((r) => r.path))).size, sampleHandle: refs[0]?.references[0] && { path: refs[0].references[0].path, kind: refs[0].references[0].kind, index: refs[0].references[0].index } };
s = t(); const resolved = refs[0].references.slice(0, 50).map((h) => h.resolve()); out.resolve50_ms = +(t() - s).toFixed(0);
out.sampleResolved = resolved.slice(0, 2).map((n) => n && { pos: n.pos, end: n.end, file: n.getSourceFile?.()?.fileName });
// incremental: change the probe file on disk and update snapshot
fs.appendFileSync(PROBE, "export const r4 = zz.boolean().safeParse(false);\n");
s = t(); snap = api.updateSnapshot({ fileChanges: { changed: [PROBE] } });
const p2 = snap.getProjects()[0]; const sf2 = p2.program.getSourceFile(SCHEMAS);
const n2 = (() => { let hit; const v = (c) => { if (c.pos <= pos && pos < c.end) { hit = c; c.forEachChild(v); } }; v(sf2); return hit; })();
const refs2 = p2.checker.getReferencedSymbolsForNode(n2, pos);
out.afterChange = { ms: +(t() - s).toFixed(0), total: refs2.reduce((a, e) => a + e.references.length, 0) };
out.timing = (({ totals }) => totals)(api.getTimingInfo());
out.goChildPeakMB = (() => { for (const d of fs.readdirSync("/proc")) { try { const c = fs.readFileSync(`/proc/${d}/cmdline`, "utf8"); if (c.includes("--api") && fs.readFileSync(`/proc/${d}/stat`, "utf8").split(") ")[1].split(" ")[1] == process.pid) return Math.round(+/VmHWM:\s+(\d+)/.exec(fs.readFileSync(`/proc/${d}/status`, "utf8"))[1] / 1024); } catch {} } })();
api.close();
fs.writeFileSync(PROBE, txt);
console.log(JSON.stringify(out, null, 2));
