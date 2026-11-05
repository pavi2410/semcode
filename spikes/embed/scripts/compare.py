"""Compare embedding dumps (*.docs.f32 / *.queries.f32) against a reference (ORT fp32).
usage: python3 -I compare.py <out_dir> <ref_prefix> <other_prefix>..."""
import sys, struct, os
D = 768
def load(p):
    b = open(p, "rb").read()
    v = struct.unpack(f"<{len(b)//4}f", b)
    return [v[i:i+D] for i in range(0, len(v), D)]
def cos(a, b):
    return sum(x*y for x, y in zip(a, b)) / ((sum(x*x for x in a) ** .5) * (sum(y*y for y in b) ** .5))
def top3(docs, q):
    return [i for _, i in sorted(((-cos(d, q), i) for i, d in enumerate(docs)))[:3]]
out, ref = sys.argv[1], sys.argv[2]
rd, rq = load(f"{out}/{ref}.docs.f32"), load(f"{out}/{ref}.queries.f32")
rtop = [top3(rd, q) for q in rq]
for o in sys.argv[3:]:
    if not os.path.exists(f"{out}/{o}.docs.f32"): continue
    od, oq = load(f"{out}/{o}.docs.f32"), load(f"{out}/{o}.queries.f32")
    c = [cos(a, b) for a, b in zip(rd, od)]
    cq = [cos(a, b) for a, b in zip(rq, oq)]
    ov = sum(len(set(a) & set(top3(od, q))) for a, q in zip(rtop, oq)) / (3 * len(rq))
    print(f"{o:12s} vs {ref}: doc cos mean {sum(c)/len(c):.4f} min {min(c):.4f}; query cos min {min(cq):.4f}; top3 overlap {ov:.0%}")
