#!/usr/bin/env python3
"""Pass through an HTTP proxy and record TLS ClientHello bytes, unchanged.

This is a diagnostic relay: client -> this listener -> upstream HTTP proxy.
It does not terminate TLS or inspect encrypted application data. Output is
private raw evidence and must stay outside the repository.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import select
import socket
import socketserver
import threading
import time
from pathlib import Path


class Relay(socketserver.ThreadingMixIn, socketserver.TCPServer):
    allow_reuse_address = True
    daemon_threads = True

    def __init__(self, address, upstream, output, timings_output=None):
        self.upstream = upstream
        self.output = output
        self.timings_output = timings_output
        self.output_lock = threading.Lock()
        super().__init__(address, Handler)

    def record(self, target: str, record: bytes) -> None:
        item = {"atUnix": time.time(), "target": target,
                "recordBytes": len(record), "recordSha256": hashlib.sha256(record).hexdigest(),
                "recordBase64": base64.b64encode(record).decode()}
        with self.output_lock:
            with self.output.open("a") as stream:
                stream.write(json.dumps(item) + "\n")

    def record_timing(self, item: dict) -> None:
        if self.timings_output is None:
            return
        with self.output_lock:
            with self.timings_output.open("a") as stream:
                stream.write(json.dumps(item) + "\n")


class Handler(socketserver.BaseRequestHandler):
    def handle(self) -> None:
        try:
            upstream = socket.create_connection(self.server.upstream, timeout=10)
        except OSError:
            return
        client = self.request
        client.settimeout(None)
        upstream.settimeout(None)
        connect_data = bytearray()
        hello_data = bytearray()
        proxy_response_data = bytearray()
        target = None
        recorded = False
        timing = {"connectRequestAt": None, "connectResponseAt": None,
                  "clientHelloAt": None, "firstServerTlsAt": None,
                  "proxyStatus": None, "clientBytes": 0, "serverBytes": 0,
                  "chunks": []}
        try:
            while True:
                ready, _, _ = select.select([client, upstream], [], [], 120)
                if not ready:
                    return
                for source in ready:
                    destination = upstream if source is client else client
                    chunk = source.recv(65536)
                    if not chunk:
                        return
                    at = time.time()
                    timing["clientBytes" if source is client else "serverBytes"] += len(chunk)
                    if self.server.timings_output is not None and len(timing["chunks"]) < 256:
                        timing["chunks"].append({"at": at,
                                                 "direction": "client" if source is client else "server",
                                                 "bytes": len(chunk)})
                    destination.sendall(chunk)
                    if source is upstream:
                        if target and timing["connectResponseAt"] is None:
                            proxy_response_data.extend(chunk)
                            boundary = proxy_response_data.find(b"\r\n\r\n")
                            if boundary >= 0:
                                first_line = proxy_response_data.split(b"\r\n", 1)[0].split(b" ")
                                timing["proxyStatus"] = first_line[1].decode("ascii", "replace") if len(first_line) > 1 else None
                                timing["connectResponseAt"] = at
                                if len(proxy_response_data) > boundary + 4:
                                    timing["firstServerTlsAt"] = at
                                proxy_response_data.clear()
                            elif len(proxy_response_data) > 16384:
                                proxy_response_data.clear()
                        elif target and timing["firstServerTlsAt"] is None:
                            timing["firstServerTlsAt"] = at
                        continue
                    if recorded:
                        continue
                    if target is None:
                        connect_data.extend(chunk)
                        boundary = connect_data.find(b"\r\n\r\n")
                        if boundary < 0:
                            if len(connect_data) > 16384:
                                return
                            continue
                        head = bytes(connect_data[:boundary]).split(b"\r\n", 1)[0]
                        parts = head.split(b" ")
                        target = parts[1].decode("ascii", "replace") if len(parts) >= 2 and parts[0] == b"CONNECT" else ""
                        timing["connectRequestAt"] = at
                        hello_data.extend(connect_data[boundary+4:])
                        connect_data.clear()
                    else:
                        hello_data.extend(chunk)
                    if len(hello_data) >= 5 and hello_data[0] == 22:
                        size = 5 + int.from_bytes(hello_data[3:5], "big")
                        if len(hello_data) >= size:
                            timing["clientHelloAt"] = at
                            if target.lower().startswith("www.southwest.com:"):
                                self.server.record(target, bytes(hello_data[:size]))
                            recorded = True
                    if len(hello_data) > 32768:
                        recorded = True
        except (OSError, ValueError):
            return
        finally:
            if target and (target.lower().endswith(".southwest.com:443") or target.lower() == "southwest.com:443"):
                self.server.record_timing({"target": target, **timing, "closedAt": time.time()})
            upstream.close()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--listen-port", type=int, default=17890)
    parser.add_argument("--upstream-host", default="127.0.0.1")
    parser.add_argument("--upstream-port", type=int, default=7890)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timings-output", type=Path,
                        help="Optional private CONNECT/TLS milestone log without payload bytes")
    args = parser.parse_args()
    args.output.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    if args.output.parent.stat().st_mode & 0o077:
        parser.error("--output parent directory must have private permissions (0700)")
    args.output.touch(mode=0o600, exist_ok=True)
    os.chmod(args.output, 0o600)
    if args.timings_output:
        args.timings_output.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        if args.timings_output.parent.stat().st_mode & 0o077:
            parser.error("--timings-output parent directory must have private permissions (0700)")
        args.timings_output.touch(mode=0o600, exist_ok=True)
        os.chmod(args.timings_output, 0o600)
    with Relay(("127.0.0.1", args.listen_port),
               (args.upstream_host, args.upstream_port), args.output, args.timings_output) as server:
        print(f"proxy tap listening on 127.0.0.1:{args.listen_port}", flush=True)
        try:
            server.serve_forever(poll_interval=0.2)
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
