#!/usr/bin/env python3
"""One-off test for ticket 18: copy a desktop Sidebar entry into an inactive Profile, then verify.
usage: synctest.py prep <local_id> | synctest.py verify"""
import hashlib, json, shutil, sys, tarfile, time
from pathlib import Path

ROOT = Path.home() / "Library/Application Support/Claude/claude-code-sessions"
SRC = ROOT / "dce123fe-6802-4cb8-acbd-3af1c7fdb36e/0e000f92-abef-478d-8c20-ba86f007e2b6"
DST = ROOT / "4bc338e1-d7ab-4be3-9f4f-dc8f53ee6994/d0ae4c16-71ec-4cb3-a9c0-83c25d493ab0"
STATE = Path(__file__).with_name("synctest-state")
CLEAR = {"bridgeSessionIds": [], "remoteMcpServersConfig": [], "enabledMcpTools": []}

def sha(p): return hashlib.sha256(p.read_bytes()).hexdigest()

def prep(local_id):
    name = f"{local_id}.json"
    src, dst = SRC / name, DST / name
    assert src.exists(), f"missing {src}"
    assert not dst.exists(), f"already in target: {dst}"
    assert not (DST / name.replace("local_", "deleted_")).exists(), "tombstone in target"
    STATE.mkdir(exist_ok=True)
    with tarfile.open(STATE / f"target-backup-{int(time.time())}.tgz", "w:gz") as t:
        t.add(DST, arcname=DST.name)
    shutil.copy2(src, STATE / "source-before.json")
    entry = json.loads(src.read_text())
    present = {k: entry.get(k) for k in CLEAR if k in entry}
    entry.update({k: v for k, v in CLEAR.items() if k in entry})
    tmp = dst.with_suffix(".tmp")
    tmp.write_text(json.dumps(entry))
    tmp.rename(dst)
    shutil.copy2(dst, STATE / "copy-as-written.json")
    (STATE / "meta.json").write_text(json.dumps({"id": local_id, "cleared": present, "source_sha": sha(src)}, indent=1))
    print("cleared:", {k: (len(v) if isinstance(v, list) else v) for k, v in present.items()})
    print("wrote", dst)

def verify():
    meta = json.loads((STATE / "meta.json").read_text())
    name = meta["id"] + ".json"
    src, dst = SRC / name, DST / name
    print("source unchanged:", src.exists() and sha(src) == meta["source_sha"])
    if src.exists() and sha(src) != meta["source_sha"]:
        print(" source diff:"); diff(json.loads((STATE / "source-before.json").read_text()), json.loads(src.read_text()))
    if not dst.exists():
        print("target entry: GONE; tombstone:", (DST / name.replace("local_", "deleted_")).exists()); return
    print(" target diff:"); diff(json.loads((STATE / "copy-as-written.json").read_text()), json.loads(dst.read_text()))

def diff(before, after):
    for k in sorted(set(before) | set(after)):
        if before.get(k) != after.get(k):
            print(f"  {k}: {str(before.get(k))[:80]!r} -> {str(after.get(k))[:80]!r}")

{"prep": lambda: prep(sys.argv[2]), "verify": verify}[sys.argv[1]]()
