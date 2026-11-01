#!/usr/bin/env python3
"""Serve the web build with caching disabled.

The wasm bindings and the wasm module must always come from the same build.
A browser that keeps the old `party_dungeon.js` while fetching the new
`party_dungeon_bg.wasm` calls missing exports and fails at startup. Sending
`no-store` on every response stops that mismatch during development.
"""

from __future__ import annotations

import os
import sys
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "web")
PORT = int(os.environ.get("PORT", "8082"))


class NoCacheHandler(SimpleHTTPRequestHandler):
    extensions_map = {
        **SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "text/javascript",
        ".mjs": "text/javascript",
        ".json": "application/json",
        ".html": "text/html",
    }

    def end_headers(self) -> None:
        self.send_header("Cache-Control", "no-store, no-cache, must-revalidate, max-age=0")
        self.send_header("Pragma", "no-cache")
        self.send_header("Expires", "0")
        super().end_headers()

    def log_message(self, fmt: str, *args: object) -> None:
        sys.stderr.write("%s - %s\n" % (self.address_string(), fmt % args))


def main() -> None:
    handler = lambda *args, **kwargs: NoCacheHandler(  # noqa: E731
        *args, directory=ROOT, **kwargs
    )
    server = ThreadingHTTPServer(("", PORT), handler)
    print(f"Serving {os.path.normpath(ROOT)} on http://localhost:{PORT}/", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
