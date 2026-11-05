#!/usr/bin/env python3
"""Generic LSP benchmark driver for the Python L2 spike.

Usage: lsp_bench.py <name> <workspace_root> <out.json> -- <server cmd...>

Runs a fixed script of requests against a Python language server rooted at a
copy of httpx (with fixture_semcode.py added) and records timings, peak RSS,
raw results and scores against a hand-written ground truth.
"""
import json, os, shutil, subprocess, sys, threading, time, queue
from urllib.parse import quote, unquote

name, root, out_path = sys.argv[1], os.path.abspath(sys.argv[2]), sys.argv[3]
cmd = sys.argv[sys.argv.index("--") + 1:]
VENV_PY = os.path.join(root, ".venv/bin/python")


def uri(rel):
    return "file://" + quote(os.path.join(root, rel))


def rel(u):
    return os.path.relpath(unquote(u[len("file://"):]), root)


class Lsp:
    def __init__(self, cmd):
        self.p = subprocess.Popen(cmd, cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=open(f"/tmp/{name}.stderr.log", "w"))
        self.id = 0
        self.pending = {}
        self.notes = []
        self.server_requests = []
        self.progress_done = threading.Event()
        self.lock = threading.Lock()
        self.peak_rss_kb = 0
        threading.Thread(target=self._reader, daemon=True).start()
        threading.Thread(target=self._rss, daemon=True).start()

    def _tree_rss(self):
        pids = [self.p.pid]
        total = 0
        i = 0
        while i < len(pids):
            pid = pids[i]; i += 1
            try:
                for line in open(f"/proc/{pid}/status"):
                    if line.startswith("VmRSS:"):
                        total += int(line.split()[1])
                for t in os.listdir(f"/proc/{pid}/task"):
                    try:
                        pids += [int(c) for c in open(f"/proc/{pid}/task/{t}/children").read().split()]
                    except OSError:
                        pass
            except OSError:
                pass
        return total

    def _rss(self):
        while self.p.poll() is None:
            self.peak_rss_kb = max(self.peak_rss_kb, self._tree_rss())
            time.sleep(0.05)

    def _send(self, msg):
        data = json.dumps(msg).encode()
        with self.lock:
            self.p.stdin.write(b"Content-Length: %d\r\n\r\n" % len(data) + data)
            self.p.stdin.flush()

    def _reader(self):
        f = self.p.stdout
        while True:
            headers = {}
            while True:
                line = f.readline()
                if not line:
                    return
                line = line.strip()
                if not line:
                    break
                k, v = line.split(b":", 1)
                headers[k.lower()] = v.strip()
            body = json.loads(f.read(int(headers[b"content-length"])))
            if "id" in body and "method" in body:
                self._on_server_request(body)
            elif "id" in body:
                q = self.pending.pop(body["id"], None)
                if q:
                    q.put(body)
            else:
                body["_t"] = round(time.perf_counter() - T0, 3)
                self.notes.append(body)
                if body.get("method") == "$/progress" and body["params"]["value"].get("kind") == "end":
                    self.progress_done.set()

    def _on_server_request(self, req):
        m = req["method"]
        self.server_requests.append(m if m != "client/registerCapability" else
                                    m + ":" + ",".join(r["method"] for r in req["params"]["registrations"]))
        result = None
        if m == "workspace/configuration":
            result = []
            for item in req["params"]["items"]:
                sec = item.get("section")
                if sec == "python":
                    result.append({"pythonPath": VENV_PY})
                elif sec in ("basedpyright.analysis", "python.analysis"):
                    result.append({"diagnosticMode": "openFilesOnly"})
                elif sec in ("basedpyright", "pyright"):
                    result.append({})
                else:
                    result.append(None)
        self._send({"jsonrpc": "2.0", "id": req["id"], "result": result})

    def request(self, method, params, timeout=300):
        self.id += 1
        q = queue.Queue()
        self.pending[self.id] = q
        t0 = time.perf_counter()
        self._send({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params})
        resp = q.get(timeout=timeout)
        dt = time.perf_counter() - t0
        return resp.get("result"), resp.get("error"), dt

    def notify(self, method, params):
        self._send({"jsonrpc": "2.0", "method": method, "params": params})


def find_pos(relpath, line_substr, token, nth_line=0, enc="utf-16"):
    """0-based (line, character) of `token` on the first line containing line_substr."""
    lines = open(os.path.join(root, relpath), encoding="utf-8").read().split("\n")
    hits = [i for i, l in enumerate(lines) if line_substr in l]
    i = hits[nth_line]
    col_cp = lines[i].index(token, lines[i].index(line_substr))
    prefix = lines[i][:col_cp]
    if enc == "utf-8":
        col = len(prefix.encode("utf-8"))
    elif enc == "utf-32":
        col = len(prefix)
    else:
        col = len(prefix.encode("utf-16-le")) // 2
    return {"line": i, "character": col}


def locs(res):
    if not res:
        return []
    return sorted({f"{rel(l['uri'])}:{l['range']['start']['line'] + 1}" for l in res})


# Ground truth (1-based lines) for references to httpx.Client.send (excluding decl).
TRUE_REFS = {
    "httpx/_client.py:825", "httpx/_client.py:868",
    "tests/client/test_client.py:54", "tests/client/test_client.py:69",
    "tests/client/test_redirects.py:143", "tests/client/test_redirects.py:148",
    "fixture_semcode.py:8",   # super().send
    "fixture_semcode.py:13",  # import httpx as h; h.Client().send
    "fixture_semcode.py:17",  # from httpx import Client as C; C().send
    "fixture_semcode.py:21",  # param annotated httpx.Client
    "fixture_semcode.py:31",  # with httpx.Client() as client
}
OVERRIDE_REFS = {"fixture_semcode.py:7", "fixture_semcode.py:26"}  # MyClient.send def + call
DECL = "httpx/_client.py:879"


def score(found):
    f = set(found) - {DECL}
    tp = f & TRUE_REFS
    return {"found": len(f), "true_pos": len(tp), "recall": f"{len(tp)}/{len(TRUE_REFS)}",
            "missed": sorted(TRUE_REFS - f), "extra": sorted(f - TRUE_REFS)}


def text_doc(relpath):
    return {"uri": uri(relpath), "languageId": "python", "version": 1,
            "text": open(os.path.join(root, relpath), encoding="utf-8").read()}


def apply_edit_copy(edit, enc, tag):
    """Apply a WorkspaceEdit to a fresh git-clean copy; return diff stat + changed lines."""
    dst = f"/tmp/{name}-rename-{tag}"
    shutil.rmtree(dst, ignore_errors=True)
    subprocess.run(["git", "clone", "-q", root, dst], check=True)
    changes = {}
    for u, edits in (edit.get("changes") or {}).items():
        changes.setdefault(u, []).extend(edits)
    for dc in edit.get("documentChanges") or []:
        if "textDocument" in dc:
            changes.setdefault(dc["textDocument"]["uri"], []).extend(dc["edits"])
    for u, edits in changes.items():
        path = os.path.join(dst, rel(u))
        lines = open(path, encoding="utf-8").read().split("\n")

        def to_cp(line, ch):
            s = lines[line]
            if enc == "utf-8":
                return len(s.encode("utf-8")[:ch].decode("utf-8"))
            if enc == "utf-16":
                return len(s.encode("utf-16-le")[:ch * 2].decode("utf-16-le"))
            return ch
        for e in sorted(edits, key=lambda e: (e["range"]["start"]["line"], e["range"]["start"]["character"]), reverse=True):
            s, en = e["range"]["start"], e["range"]["end"]
            assert s["line"] == en["line"]
            l = lines[s["line"]]
            a, b = to_cp(s["line"], s["character"]), to_cp(en["line"], en["character"])
            lines[s["line"]] = l[:a] + e["newText"] + l[b:]
        open(path, "w", encoding="utf-8").write("\n".join(lines))
    diff = subprocess.run(["git", "-C", dst, "diff", "-U0"], capture_output=True, text=True).stdout
    changed = [l for l in diff.splitlines() if l.startswith("+") and not l.startswith("+++")]
    return dst, changed, sorted(f"{rel(u)}:{e['range']['start']['line'] + 1}" for u, es in changes.items() for e in es)


T0 = time.perf_counter()
WDP = os.environ.get("WDP") == "1"                      # enable workDoneProgress + wait for end
ENCS = os.environ.get("ENCODINGS", "utf-8,utf-16").split(",")
NOTIFY_WATCH = os.environ.get("NO_WATCH_NOTIFY") != "1"
R = {"server": name, "cmd": cmd, "opts": {"WDP": WDP, "ENCODINGS": ENCS, "NOTIFY_WATCH": NOTIFY_WATCH}}
t_start = time.perf_counter()
s = Lsp(cmd)
init, err, dt = s.request("initialize", {
    "processId": os.getpid(), "rootUri": uri(""),
    "workspaceFolders": [{"uri": uri(""), "name": "httpx"}],
    "capabilities": {
        "general": {"positionEncodings": ENCS},
        "textDocument": {
            "hover": {"contentFormat": ["plaintext", "markdown"]},
            "rename": {"prepareSupport": True},
            "callHierarchy": {"dynamicRegistration": False},
            "references": {}, "definition": {}, "synchronization": {"didSave": True},
        },
        "workspace": {"workspaceEdit": {"documentChanges": True}, "configuration": True,
                      "workspaceFolders": True,
                      "didChangeWatchedFiles": {"dynamicRegistration": True}},
        "window": {"workDoneProgress": WDP},
    },
    "initializationOptions": {},
})
s.notify("initialized", {})
caps = init["capabilities"]
enc = caps.get("positionEncoding", "utf-16")
R["server_info"] = init.get("serverInfo")
R["t_initialize_s"] = round(time.perf_counter() - t_start, 3)
R["position_encoding_negotiated"] = enc
R["capabilities"] = caps
R["feature_support"] = {k: bool(caps.get(k)) for k in [
    "definitionProvider", "referencesProvider", "renameProvider", "hoverProvider",
    "callHierarchyProvider", "typeHierarchyProvider", "implementationProvider",
    "workspaceSymbolProvider", "documentSymbolProvider", "semanticTokensProvider",
    "inlayHintProvider", "codeActionProvider", "typeDefinitionProvider", "declarationProvider"]}

CL = "httpx/_client.py"
FX = "fixture_semcode.py"
s.notify("textDocument/didOpen", {"textDocument": text_doc(CL)})
s.notify("textDocument/didOpen", {"textDocument": text_doc(FX)})
def_send = find_pos(CL, "    def send(", "send", enc=enc)
if WDP:
    ok = s.progress_done.wait(120)
    R["wait_progress_end_s"] = round(time.perf_counter() - t_start, 3) if ok else None

# Cold / warm references from the declaration.
res, err, dt = s.request("textDocument/references", {"textDocument": {"uri": uri(CL)}, "position": def_send,
                                                     "context": {"includeDeclaration": False}})
R["refs_cold_s"] = round(dt, 3)
R["t_first_refs_since_spawn_s"] = round(time.perf_counter() - t_start, 3)
R["refs_from_decl"] = locs(res); R["refs_from_decl_error"] = err
R["refs_from_decl_score"] = score(locs(res))
res2, _, dt = s.request("textDocument/references", {"textDocument": {"uri": uri(CL)}, "position": def_send,
                                                    "context": {"includeDeclaration": True}})
R["refs_warm_s"] = round(dt, 3); R["refs_warm_score"] = score(locs(res2))
res3, _, dt = s.request("textDocument/references", {"textDocument": {"uri": uri(CL)}, "position": def_send,
                                                    "context": {"includeDeclaration": False}})
R["refs_warm_nodecl_s"] = round(dt, 3); R["refs_warm_nodecl_score"] = score(locs(res3))

# References from alias use site (h.Client() ... c.send).
p = find_pos(FX, "return c.send(req)", "send", enc=enc)
res, err, dt = s.request("textDocument/references", {"textDocument": {"uri": uri(FX)}, "position": p,
                                                     "context": {"includeDeclaration": True}})
R["refs_from_alias_site"] = score(locs(res)); R["refs_from_alias_site_s"] = round(dt, 3)

# Position-encoding check: line 32 has 'é😀' before the call.
p32 = find_pos(FX, 's = "é', "send", enc=enc)
hit = [l for l in (res2 or []) if rel(l["uri"]) == FX and l["range"]["start"]["line"] == p32["line"]]
R["encoding_check"] = {"expected_char_in_" + enc: p32["character"],
                       "expected_utf16": find_pos(FX, 's = "é', "send", enc="utf-16")["character"],
                       "expected_utf8": find_pos(FX, 's = "é', "send", enc="utf-8")["character"],
                       "server_returned": [h["range"]["start"]["character"] for h in hit]}

# Hover / type of expression.
hovers = {}
for label, (f, sub, tok) in {
    "client_in_with": (FX, "with httpx.Client() as client", "client"),
    "r_result_of_send": (FX, 's = "é', "r"),
    "c_alias_instance": (FX, "c = h.Client()", "c"),
    "self_send_in_lib": (CL, "return self.send(request, auth=auth", "send"),
}.items():
    pos = find_pos(f, sub, tok, enc=enc)
    if tok == "r":
        pos = find_pos(f, sub, "r =", enc=enc)
    h, err, dt = s.request("textDocument/hover", {"textDocument": {"uri": uri(f)}, "position": pos})
    c = (h or {}).get("contents")
    txt = c.get("value") if isinstance(c, dict) else json.dumps(c)
    hovers[label] = {"s": round(dt, 3), "text": (txt or "")[:300], "error": err}
R["hover"] = hovers

# Call hierarchy incoming.
items, err, dt = s.request("textDocument/prepareCallHierarchy", {"textDocument": {"uri": uri(CL)}, "position": def_send})
R["call_hierarchy_prepare"] = {"s": round(dt, 3), "error": err, "items": [i["name"] for i in items or []]}
if items:
    inc, err, dt = s.request("callHierarchy/incomingCalls", {"item": items[0]})
    R["call_hierarchy_incoming"] = {"s": round(dt, 3), "error": err,
        "callers": sorted(f"{rel(c['from']['uri'])}:{c['from']['name']}" for c in inc or [])}

# Rename from base method and from subclass override.
for tag, (f, sub) in {"base": (CL, "    def send("), "override": (FX, "    def send(")}.items():
    pos = find_pos(f, sub, "send", enc=enc)
    prep, perr, _ = s.request("textDocument/prepareRename", {"textDocument": {"uri": uri(f)}, "position": pos})
    edit, err, dt = s.request("textDocument/rename", {"textDocument": {"uri": uri(f)}, "position": pos, "newName": "send_req"})
    entry = {"s": round(dt, 3), "prepare_ok": prep is not None, "prepare_error": perr, "error": err}
    if edit:
        dst, changed, edited = apply_edit_copy(edit, enc, tag)
        entry.update({"edited_locations": edited, "n_edits": len(edited), "copy": dst,
                      "touches_override": any(e.startswith(FX + ":7") for e in edited),
                      "touches_base_decl": DECL in edited,
                      "touches_async_client": any(e in edited for e in ("httpx/_client.py:1540", "httpx/_client.py:1583", "httpx/_client.py:1594"))})
    R["rename_" + tag] = entry

# Incremental 1: didChange on open document adds a new call.
src = open(os.path.join(root, FX), encoding="utf-8").read()
new_src = src + "\n\ndef added_by_edit(req: httpx.Request) -> httpx.Response:\n    return httpx.Client().send(req)\n"
s.notify("textDocument/didChange", {"textDocument": {"uri": uri(FX), "version": 2},
                                    "contentChanges": [{"text": new_src}]})
res, err, dt = s.request("textDocument/references", {"textDocument": {"uri": uri(CL)}, "position": def_send,
                                                     "context": {"includeDeclaration": False}})
new_line = new_src.count("\n")  # 1-based line of the new call
R["incr_didChange"] = {"s": round(dt, 3), "sees_new_call": f"{FX}:{new_line}" in locs(res), "n": len(locs(res))}

# Incremental 2: on-disk new file (never opened) + didChangeWatchedFiles.
newf = os.path.join(root, "fixture_ondisk.py")
open(newf, "w").write("import httpx\n\ndef ondisk(req: httpx.Request):\n    return httpx.Client().send(req)\n")
if NOTIFY_WATCH:
    s.notify("workspace/didChangeWatchedFiles", {"changes": [{"uri": uri("fixture_ondisk.py"), "type": 1}]})
time.sleep(float(os.environ.get("DISK_WAIT", "1.0")))
res, err, dt = s.request("textDocument/references", {"textDocument": {"uri": uri(CL)}, "position": def_send,
                                                     "context": {"includeDeclaration": False}})
R["incr_ondisk_new_file"] = {"s": round(dt, 3), "sees_new_call": "fixture_ondisk.py:4" in locs(res)}
os.remove(newf)

R["peak_rss_mb_tree"] = round(s.peak_rss_kb / 1024, 1)
R["server_to_client_requests"] = s.server_requests
R["notifications"] = [{"t": n["_t"], "method": n.get("method"),
                       "msg": json.dumps(n.get("params"))[:200]} for n in s.notes
                      if n.get("method") in ("window/logMessage", "$/progress", "window/showMessage")]
s.request("shutdown", None, timeout=30)
s.notify("exit", None)
json.dump(R, open(out_path, "w"), indent=2)
summary = {k: v for k, v in R.items() if k not in ("capabilities",)}
print(json.dumps(summary, indent=1)[:6000])
