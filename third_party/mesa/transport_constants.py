"""Extract fixture command ordinals from the pinned Mesa Venus protocol.

The fixture verifies the transport before the full Mesa renderer is initialized.
Command encoding follows vn_protocol_driver_{types,transport,instance}.h.
"""
from pathlib import Path
import re
import sys

root = Path(sys.argv[1]).parent
header = (root / "src/virtio/venus-protocol/vn_protocol_driver_defines.h").read_text()
names = {
    "SET_REPLY_STREAM": "VK_COMMAND_TYPE_vkSetReplyCommandStreamMESA_EXT",
    "ENUMERATE_INSTANCE_VERSION": "VK_COMMAND_TYPE_vkEnumerateInstanceVersion_EXT",
    "GENERATE_REPLY": "VK_COMMAND_GENERATE_REPLY_BIT_EXT",
}
output = ["#![no_std]\n"]
for name, source in names.items():
    matches = re.findall(r"\b" + source + r"\s*=\s*(0x[0-9a-fA-F]+|[0-9]+)\s*,", header)
    if len(matches) != 1:
        raise ValueError("missing or ambiguous Venus command: " + source)
    output.append(f"pub const {name}: u32 = {int(matches[0], 0)};\n")
Path(sys.argv[2]).write_text("".join(output))
