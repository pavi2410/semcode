#!/usr/bin/env python3
"""Option B spike: drive a rust-analyzer binary over LSP stdio with a hand-rolled
JSON-RPC client (stdlib only) and time the semcode L2 queries on ripgrep.

usage: python3 -I ra_lsp_bench.py <rust-analyzer-binary> <ripgrep-root> [utf-8|utf-16]
"""
import json, os, subprocess, sys, threading, time, queue
from pathlib import Path

RA, ROOT = sys.argv[1], Path(sys.argv[2]).resolve()
ENC = sys.argv[3] if len(sys.argv) > 3 else "utf-8"


class Lsp:
    def __init__(self, cmd, cwd):
        self.p = subprocess.Popen(cmd, cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=open("/tmp/ra-lsp-stderr.log", "wb"))
        self.nid = 0
        self.pending = {}
        self.notes = queue.Queue()
        threading.Thread(target=self._reader, daemon=True).start()

    def _reader(self):
        f = self.p.stdout
        while True:
            hdr = {}
            while True:
                line = f.readline()
                if not line:
                    return
                line = line.decode().strip()
                if not line:
                    break
                k, v = line.split(":", 1)
                hdr[k.lower()] = v.strip()
            msg = json.loads(f.read(int(hdr["content-length"])))
            if "id" in msg and "method" in msg:  # server->client request
                self._send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
            elif "id" in msg:
                self.pending.pop(msg["id"]).put(msg)
            else:
                self.notes.put(msg)

    def _send(self, obj):
        b = json.dumps(obj).encode()
        self.p.stdin.write(b"Content-Length: %d\r\n\r\n" % len(b) + b)
        self.p.stdin.flush()

    def req(self, method, params):
        self.nid += 1
        q = queue.Queue()
        self.pending[self.nid] = q
        self._send({"jsonrpc": "2.0", "id": self.nid, "method": method, "params": params})
        m = q.get()
        if "error" in m:
            raise RuntimeError(f"{method}: {m['error']}")
        return m["result"]

    def notify(self, method, params):
        self._send({"jsonrpc": "2.0", "method": method, "params": params})


def rss_kb(pid):
    """RSS of pid + descendants (proc-macro-srv), in KB."""
    total, hwm = 0, 0
    pids = [pid]
    try:
        out = subprocess.run(["pgrep", "-P", str(pid)], capture_output=True, text=True).stdout
        pids += [int(x) for x in out.split()]
    except Exception:
        pass
    for p in pids:
        try:
            for line in open(f"/proc/{p}/status"):
                if line.startswith("VmRSS:"):
                    total += int(line.split()[1])
                if line.startswith("VmHWM:"):
                    hwm += int(line.split()[1])
        except FileNotFoundError:
            pass
    return total, hwm


def uri(rel):
    return (ROOT / rel).as_uri()


def pos(rel, line1, needle, nth=0):
    """0-based LSP position of `needle` on 1-based line, in the negotiated encoding."""
    text = (ROOT / rel).read_text().splitlines()[line1 - 1]
    col = -1
    for _ in range(nth + 1):
        col = text.index(needle, col + 1)
    prefix = text[:col]
    ch = len(prefix.encode("utf-8")) if NEG == "utf-8" else len(prefix.encode("utf-16-le")) // 2
    return {"textDocument": {"uri": uri(rel)}, "position": {"line": line1 - 1, "character": ch}}


def timed(label, fn):
    t = time.perf_counter()
    r = fn()
    dt = (time.perf_counter() - t) * 1000
    print(f"  [{dt:8.1f} ms] {label}")
    return r, dt


def short(loc):
    u = loc.get("uri") or loc.get("targetUri")
    r = loc.get("range") or loc.get("targetSelectionRange")
    return f"{u.replace(ROOT.as_uri() + '/', '')}:{r['start']['line'] + 1}"


t0 = time.perf_counter()
lsp = Lsp([RA], str(ROOT))
init = lsp.req("initialize", {
    "processId": os.getpid(),
    "rootUri": ROOT.as_uri(),
    "workspaceFolders": [{"uri": ROOT.as_uri(), "name": "ripgrep"}],
    "capabilities": {
        "general": {"positionEncodings": [ENC]},
        "textDocument": {"hover": {"contentFormat": ["plaintext"]},
                         "rename": {"prepareSupport": True},
                         "callHierarchy": {}},
        "workspace": {"workspaceEdit": {"documentChanges": True}},
        "window": {"workDoneProgress": True},
        "experimental": {"serverStatusNotification": True, "hoverRange": True},
    },
    "initializationOptions": {"cachePriming": {"enable": True}, "checkOnSave": False,
                              "diagnostics": {"enable": "--no-diagnostics" not in sys.argv},
                              "files": {"watcher": "server" if "--server-watcher" in sys.argv else "client"},
                              "cargo": {"features": "all" if "--all-features" in sys.argv else []}},
})
NEG = init["capabilities"].get("positionEncoding", "utf-16")
print(f"server: {init.get('serverInfo')}, negotiated positionEncoding={NEG}")
lsp.notify("initialized", {})

# Cold load: track $/progress tokens and serverStatus; done = quiescent AND no
# active progress (cache priming reports via $/progress, not serverStatus).
peak = 0
active, seen_busy, quiescent = {}, False, False
while True:
    try:
        n = lsp.notes.get(timeout=1)
    except queue.Empty:
        peak = max(peak, rss_kb(lsp.p.pid)[0])
        if quiescent and not active and seen_busy:
            break
        continue
    el = time.perf_counter() - t0
    if n["method"] == "$/progress":
        v = n["params"]["value"]
        tok = n["params"]["token"]
        if v["kind"] == "begin":
            active[tok] = v.get("title")
            seen_busy = True
        elif v["kind"] == "end":
            print(f"  t={el:6.1f}s progress done: {active.pop(tok, tok)}")
    elif n["method"] == "experimental/serverStatus":
        st = n["params"]
        if not st.get("quiescent"):
            seen_busy = True
        quiescent = st.get("quiescent") and seen_busy
        print(f"  t={el:6.1f}s serverStatus quiescent={st.get('quiescent')} health={st.get('health')} msg={(st.get('message') or '')[:200]!r}")
cold = time.perf_counter() - t0
print(f"cold load to quiescent (incl. cargo metadata, build scripts, sysroot, cache priming): {cold:.1f}s")
print(f"RSS after load: {rss_kb(lsp.p.pid)[0] / 1024:.0f} MB (peak sampled {peak / 1024:.0f} MB)")

MATCHER = ("crates/matcher/src/lib.rs", 546, "Matcher")
SEARCH_PATH = ("crates/searcher/src/searcher/mod.rs", 643, "search_path")
ALIAS_USE = ("crates/core/flags/defs.rs", 1256, "Separator")
FIND_AT = ("crates/matcher/src/lib.rs", 571, "find_at")
SINK_EXPR = ("crates/core/search.rs", 388, "sink")
open_files = set()


def did_open(rel):
    if rel in open_files:
        return
    open_files.add(rel)
    lsp.notify("textDocument/didOpen", {"textDocument": {
        "uri": uri(rel), "languageId": "rust", "version": 1, "text": (ROOT / rel).read_text()}})


if "--no-open" not in sys.argv:  # RA answers for unopened files from its own VFS
    for f in {MATCHER[0], SEARCH_PATH[0], ALIAS_USE[0], SINK_EXPR[0]}:
        did_open(f)


def refs(target, label):
    r, dt = timed(label, lambda: lsp.req("textDocument/references",
                                          {**pos(*target), "context": {"includeDeclaration": False}}))
    files = {short(x).rsplit(":", 1)[0] for x in r}
    print(f"      -> {len(r)} refs in {len(files)} files")
    return r


print("queries (first call, then warm repeat):")
refs(MATCHER, "references Matcher trait")
refs(MATCHER, "references Matcher trait (warm)")
refs(SEARCH_PATH, "references Searcher::search_path")

d, _ = timed("goto-definition through `use ContextSeparator as Separator`",
             lambda: lsp.req("textDocument/definition", pos(*ALIAS_USE)))
print("      ->", [short(x) for x in d])

h, _ = timed("hover (type) of `sink` local in core/search.rs",
             lambda: lsp.req("textDocument/hover", pos(*SINK_EXPR)))
print("      ->", " ".join(h["contents"]["value"].split())[:160])
p = pos("crates/core/search.rs", 388, "p.sink_with_path")
txt = (ROOT / "crates/core/search.rs").read_text().splitlines()[387]
end = txt.index(");") + 1
rng = {"start": p["position"], "end": {"line": 387, "character": end}}
h, _ = timed("hoverRange type of expression `p.sink_with_path(&matcher, path)`",
             lambda: lsp.req("textDocument/hover", {"textDocument": p["textDocument"], "position": rng}))
print("      ->", " ".join(h["contents"]["value"].split())[:160] if h else None)

items, _ = timed("callHierarchy/prepare Searcher::search_path",
                 lambda: lsp.req("textDocument/prepareCallHierarchy", pos(*SEARCH_PATH)))
inc, _ = timed("callHierarchy/incomingCalls",
               lambda: lsp.req("callHierarchy/incomingCalls", {"item": items[0]}))
print("      ->", [(c["from"]["name"], len(c["fromRanges"])) for c in inc])

pr, _ = timed("prepareRename Matcher::find_at", lambda: lsp.req("textDocument/prepareRename", pos(*FIND_AT)))
we, _ = timed("rename Matcher::find_at -> find_at_renamed",
              lambda: lsp.req("textDocument/rename", {**pos(*FIND_AT), "newName": "find_at_renamed"}))
by_file = {}
for dc in we.get("documentChanges", []):
    by_file[dc["textDocument"]["uri"].replace(ROOT.as_uri() + "/", "")] = len(dc["edits"])
crates = sorted({f.split("/")[1] for f in by_file if f.startswith("crates/")})
print(f"      -> {sum(by_file.values())} edits in {len(by_file)} files, crates={crates}")
for f, n in sorted(by_file.items()):
    print(f"         {f}: {n}")
if "--apply-rename" in sys.argv:
    for dc in we["documentChanges"]:
        path = Path(dc["textDocument"]["uri"][len("file://"):])
        data = path.read_bytes()
        lines = data.split(b"\n")
        starts = [0]
        for ln in lines[:-1]:
            starts.append(starts[-1] + len(ln) + 1)
        def off(p):  # utf-8 negotiated -> character is a byte offset
            assert NEG == "utf-8"
            return starts[p["line"]] + p["character"]
        for e in sorted(dc["edits"], key=lambda e: off(e["range"]["start"]), reverse=True):
            data = data[:off(e["range"]["start"])] + e["newText"].encode() + data[off(e["range"]["end"]):]
        path.write_bytes(data)
    print("      applied rename edits to disk")

# Incremental: edit core/search.rs in-memory (didChange) adding a new caller of
# Searcher::search_path, then re-ask incoming calls. No reload.
rel = "crates/core/search.rs"
src = (ROOT / rel).read_text()
new_src = src + "\nfn spike_new_caller(s: &mut grep::searcher::Searcher, m: &grep::regex::RegexMatcher) {\n" \
                "    let mut sink = grep::searcher::sinks::UTF8(|_, _| Ok(true));\n" \
                "    let _ = s.search_path(m, std::path::Path::new(\"x\"), &mut sink);\n}\n"
if "--disk-edit" in sys.argv:  # write to disk, no didChange; rely on file watching
    (ROOT / rel).write_text(new_src)
    time.sleep(3)
else:
    did_open(rel)
    lsp.notify("textDocument/didChange", {"textDocument": {"uri": uri(rel), "version": 2},
                                          "contentChanges": [{"text": new_src}]})
inc2, _ = timed("incomingCalls after didChange (incremental)",
                lambda: lsp.req("callHierarchy/incomingCalls", {"item": items[0]}))
print("      ->", [(c["from"]["name"], len(c["fromRanges"])) for c in inc2])
refs(MATCHER, "references Matcher trait after edit")

print(f"RSS at end: {rss_kb(lsp.p.pid)[0] / 1024:.0f} MB, HWM {rss_kb(lsp.p.pid)[1] / 1024:.0f} MB")
lsp.req("shutdown", None)
lsp.notify("exit", None)
lsp.p.wait(timeout=10)
