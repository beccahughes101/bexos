"""Preserve the RPMB handover fix in the separately built native helper."""
from pathlib import Path
import re
import sys

source, output = map(Path, sys.argv[1:])
text = source.read_text()
pattern = r"(?m)^([ \t]*)if \(s->res_count <= 0\) \{"
matches = list(re.finditer(pattern, text))
if len(matches) != 1:
    raise SystemExit("pinned RPMB response handling changed; review the handover patch")
match = matches[0]
indent = match[1]
# A zero response count releases a connection during handover. It is not an
# RPMB command and must leave the authenticated backing device available.
replacement = (f"{indent}if (s->res_count == 0) {{\n"
               f"{indent}    return 0;\n{indent}}}\n" + match[0])
output.write_text(text[:match.start()] + replacement + text[match.end():])
