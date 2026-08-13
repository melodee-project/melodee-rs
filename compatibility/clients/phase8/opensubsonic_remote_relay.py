#!/usr/bin/env python3
"""Loopback relay that injects ``X-Forwarded-For: 10.99.99.99``.

The captured OpenSubsonic auth bypass trusts loopback client addresses
(legacy behavior: local-network requests skip credential checks, so no
principal is created). Real clients connect from remote addresses. This
relay lets a browser client on the same host exercise the authenticated
surface exactly as a remote client would: it listens on loopback (trusted
proxy per ``MELODEE__NATIVE__TRUSTED_PROXIES``) and forwards to the fixture
server with a fixed TEST-NET XFF, making the effective client address
non-loopback. Phase 0's capture harness defeated the bypass the same way.

Usage: opensubsonic_remote_relay.py <listen-port> <target-port>
"""

import http.server
import socketserver
import sys
import urllib.request

INJECTED_CLIENT = "10.99.99.99"


class RelayHandler(http.server.BaseHTTPRequestHandler):
    target_port: int = 0
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass

    def _relay(self, method: str):
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else None
        request = urllib.request.Request(
            f"http://127.0.0.1:{self.target_port}{self.path}",
            data=body,
            method=method,
        )
        for name, value in self.headers.items():
            if name.lower() in ("host", "content-length", "connection", "transfer-encoding"):
                continue
            request.add_header(name, value)
        request.add_header("X-Forwarded-For", INJECTED_CLIENT)
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                payload = response.read()
                self.send_response(response.status)
                for name, value in response.headers.items():
                    if name.lower() in ("connection", "transfer-encoding", "content-length"):
                        continue
                    self.send_header(name, value)
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                if self.command != "HEAD":
                    self.wfile.write(payload)
        except urllib.error.HTTPError as error:
            payload = error.read()
            self.send_response(error.code)
            for name, value in (error.headers or {}).items():
                if name.lower() in ("connection", "transfer-encoding", "content-length"):
                    continue
                self.send_header(name, value)
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(payload)

    def do_GET(self):
        self._relay("GET")

    def do_HEAD(self):
        self._relay("HEAD")

    def do_POST(self):
        self._relay("POST")

    def do_PUT(self):
        self._relay("PUT")

    def do_DELETE(self):
        self._relay("DELETE")


class ThreadingServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True


def main():
    listen_port = int(sys.argv[1])
    target_port = int(sys.argv[2])
    RelayHandler.target_port = target_port
    server = ThreadingServer(("127.0.0.1", listen_port), RelayHandler)
    server.serve_forever()


if __name__ == "__main__":
    main()
