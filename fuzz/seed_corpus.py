"""Write small synthetic inputs for the protocol fuzz target."""
from pathlib import Path

corpus = Path(__file__).parent / "corpus" / "protocol"
corpus.mkdir(parents=True, exist_ok=True)
seeds = {
    "handshake": (
        b"\0RFB 003.008\n\x01\x01\0\0\0\0\0\x01\0\x01"
        + bytes([32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 0, 8, 16, 0, 0, 0])
        + bytes(4)
    ),
    "raw": bytes([2, 0, 0, 0, 1, 2, 3, 255]),
    "tight": bytes([2, 1, 0, 0, 128, 1, 2, 3]),
    "zrle": bytes([2, 3, 0, 0, 1, 1, 2, 3]),
    "cursor": bytes([2, 4, 0, 0, 1, 2, 3, 255, 128]),
    "bell": bytes([1, 2]),
}
for name, data in seeds.items():
    (corpus / name).write_bytes(data)
