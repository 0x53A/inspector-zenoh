#!/usr/bin/env python3
"""Serve the web build with the isolation headers required by WASM threads."""

import http.server
import pathlib
import sys


class IsolatedHandler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        super().end_headers()


port = int(sys.argv[1]) if len(sys.argv) > 1 else 8090
directory = pathlib.Path(__file__).resolve().parent
handler = lambda *args, **kwargs: IsolatedHandler(  # noqa: E731
    *args, directory=str(directory), **kwargs
)
print(f"Serving Inspector Zenoh on http://localhost:{port}")
http.server.ThreadingHTTPServer(("", port), handler).serve_forever()
