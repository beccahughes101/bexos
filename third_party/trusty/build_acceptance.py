#!/usr/bin/env python3
"""Prepare an isolated AuthMgr protocol acceptance image inside a Bazel action."""
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys

recipe, test_main, *arguments = sys.argv[1:]
source, _, output = map(Path, arguments[:3])
app = source / "trusty/user/app/bexos/authmgr_acceptance"
app.mkdir(parents=True, exist_ok=True)
for path in Path(test_main).parent.iterdir():
    if path.is_file():
        shutil.copyfile(path, app / path.name)
manifest = (app / "manifest.prototxt").read_text()
fields = dict(re.findall(r'^(\w+):\s*(.+)$', manifest, re.MULTILINE))
manifest = {key: (json.loads(value) if value.startswith('"') else int(value))
            for key, value in fields.items()}
(app / "manifest.json").write_text(json.dumps(manifest))

# Add one real upstream trusted service and a client that invokes it through
# AuthMgr FE/BE. None of these changes are inputs to the standard image.
fe = source / "trusty/user/app/authmgr/authmgr-fe/lib.rs"
text = fe.read_text().replace("const PORT_COUNT: usize = 2;", "const PORT_COUNT: usize = 3;")
text = text.replace("    Manager::<_, _, PORT_COUNT, CONNECTION_COUNT>",
    '    add_server_to_authmgr_dispatcher(&mut dispatcher, "android.trusty.trustedhal.IHelloWorld/default", c"com.android.trusty.rust.hello.service.V1")?;\n'
    "    Manager::<_, _, PORT_COUNT, CONNECTION_COUNT>")
fe.write_text(text)
# The third Binder accessor adds both dispatcher stack frames and allocations.
# Give the acceptance-only FE the same bounds as the Binder test client.
fe_manifest = source / "trusty/user/app/authmgr/authmgr-fe/app/manifest.json"
fields = dict(re.findall(r'^(\w+):\s*(.+)$',
    (Path(test_main).parent / "fe_manifest.prototxt").read_text(), re.MULTILINE))
fe_manifest.write_text(json.dumps({key: (json.loads(value) if value.startswith('"') else int(value))
    for key, value in fields.items()}))
hello = source / "trusty/user/app/sample/rust-hello-world-trusted-hal"
for path in hello.rglob("*.rs"):
    path.write_text(path.read_text().replace("std::os::fd::", "binder::fd_compat::"))

server = hello / "lib/src/server.rs"
text = server.read_text()
text = text.replace("    let cb_per_session = move |uuid| {", """    let cb_per_session = move |uuid| {
        let authmgr = tipc::Uuid::new(0xf4768956, 0x62d9, 0x4904,
            [0x95, 0x12, 0x86, 0xdf, 0x36, 0x0d, 0x8d, 0x50]);
        if uuid != authmgr { return None; }
""")
server.write_text(text)

text = Path(recipe).read_text()
needle = "\ttrusty/user/app/bexos/orchestrator \\\n"
assert text.count(needle) == 1
text = text.replace(needle, needle +
    "\ttrusty/user/app/sample/rust-hello-world-trusted-hal/app \\\n"
    "\ttrusty/user/app/bexos/authmgr_acceptance \\\n")
output.mkdir(parents=True, exist_ok=True)
prepared = output / "acceptance_recipe.sh"
prepared.write_text(text)
subprocess.run(["bash", str(prepared), *arguments], check=True)
