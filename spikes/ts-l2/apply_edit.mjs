// Apply an LSP WorkspaceEdit saved by lsp_probe.mjs, honoring the negotiated position encoding.
// usage: node apply_edit.mjs results/<server>.rename.json
import fs from "node:fs";
import { fileURLToPath } from "node:url";
const { encoding, edit } = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const byUri = edit.documentChanges ? Object.fromEntries(edit.documentChanges.map((d) => [d.textDocument.uri, d.edits])) : edit.changes;
function offset(lines, lineStarts, text, p) {
  const ls = lineStarts[p.line], lt = lines[p.line];
  if (encoding === "utf-16") return ls + p.character;
  if (encoding === "utf-8") { // convert utf-8 byte col -> utf-16 index
    const b = Buffer.from(lt, "utf8").subarray(0, p.character).toString("utf8"); return ls + b.length; }
  throw new Error(encoding);
}
let n = 0;
for (const [uri, edits] of Object.entries(byUri)) {
  const f = fileURLToPath(uri); let text = fs.readFileSync(f, "utf8");
  const lines = text.split("\n"); const starts = []; let acc = 0; for (const l of lines) { starts.push(acc); acc += l.length + 1; }
  const abs = edits.map((e) => [offset(lines, starts, text, e.range.start), offset(lines, starts, text, e.range.end), e.newText]).sort((a, b) => b[0] - a[0]);
  for (const [s, e, t] of abs) { text = text.slice(0, s) + t + text.slice(e); n++; }
  fs.writeFileSync(f, text);
}
console.log(`applied ${n} edits in ${Object.keys(byUri).length} files (${encoding})`);
