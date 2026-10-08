#!/usr/bin/env python3
"""Verify a controlled Codex against loopback HTTP using synthetic native credentials."""
import argparse
import base64
import json
import os
from pathlib import Path
import select
import ssl
import shlex
import signal
import socket
import urllib.request
import urllib.error
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def encode(value):
    return base64.urlsafe_b64encode(json.dumps(value).encode()).decode().rstrip("=")


class RPC:
    def __init__(self, program, home, endpoint, certificate):
        env = {key: value for key, value in os.environ.items() if not key.startswith("OTEL_") and key.lower() not in ("http_proxy", "https_proxy", "all_proxy", "no_proxy")}
        for key in ("CODEX_ACCESS_TOKEN", "CODEX_API_KEY", "OPENAI_API_KEY", "CODEX_REFRESH_TOKEN_URL_OVERRIDE"):
            env.pop(key, None)
        env.update(CODEX_HOME=str(home), CODEX_CA_CERTIFICATE=str(certificate), LLMPROXY_CODEX_PROXY_V1="1", RUST_LOG="off",
                   NO_PROXY="*")
        self.process = subprocess.Popen([str(program), "-c", 'chatgpt_base_url="' + endpoint + '"',
                                         "-c", "respect_system_proxy=false", "app-server", "--stdio"],
                                        cwd=home, env=env, stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.buffer = b""
        try:
            self.send(1, "initialize", {"clientInfo": {"name": "llmproxy-runtime-check", "version": "1"},
                                        "capabilities": {"experimentalApi": True}})
            assert "result" in self.response(1), "initialize rejected"
            self.process.stdin.write(b'{"method":"initialized"}\n')
            self.process.stdin.flush()
            self.send(2, "llmproxy/capabilities", {})
            capabilities = self.response(2)["result"]
            assert capabilities == {"policyVersion": 1, "stateless": True,
                                    "clientToolsOnly": True, "nativeAuthReady": True}, "policy not confirmed: " + json.dumps(capabilities)
        except BaseException:
            self.close(crash=True)
            raise

    def send(self, id, method, params):
        self.process.stdin.write(json.dumps({"id": id, "method": method, "params": params}).encode() + b"\n")
        self.process.stdin.flush()

    def read(self):
        deadline = time.monotonic() + 30
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            assert remaining > 0 and select.select([self.process.stdout], [], [], remaining)[0], "RPC timeout"
            part = os.read(self.process.stdout.fileno(), 65536)
            assert part, "unexpected RPC EOF"
            self.buffer += part
            assert len(self.buffer) <= 17 * 1024 * 1024, "oversized RPC frame"
        line, self.buffer = self.buffer.split(b"\n", 1)
        return json.loads(line)

    def response(self, id):
        while True:
            value = self.read()
            if value.get("id") == id:
                return value
            assert "id" not in value, "unexpected response ID"

    def body(self, id, failed=False):
        body = bytearray()
        while True:
            value = self.read()
            if value.get("method") != "llmproxy/chunk":
                assert "id" not in value, "unexpected response ID"
                continue
            chunk = value["params"]
            assert chunk["requestId"] == id
            if chunk["end"]:
                assert chunk["error"] == failed and not chunk["data"]
                return bytes(body)
            assert not chunk["error"]
            body.extend(base64.b64decode(chunk["data"], validate=True))

    def close(self, crash=False):
        if self.process.poll() is None:
            if crash:
                self.process.kill()
            else:
                self.process.stdin.close()
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=10)


def verify_agent(program, codex, home, endpoint, certificate, request):
    wrapper = home / "controlled-codex"
    wrapper.write_text("#!/bin/sh\nexec " + shlex.quote(str(codex)) + " -c " +
                       shlex.quote('chatgpt_base_url="' + endpoint + '"') +
                       ' -c respect_system_proxy=false "$@"\n')
    wrapper.chmod(0o700)
    key = home / "local-key"
    key.write_text("synthetic-local-api-key" * 3)
    key.chmod(0o600)
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    env = {name: value for name, value in os.environ.items()
           if not name.startswith("OTEL_") and name.lower() not in ("http_proxy", "https_proxy", "all_proxy", "no_proxy")
           and name not in ("CODEX_ACCESS_TOKEN", "CODEX_API_KEY", "OPENAI_API_KEY", "CODEX_REFRESH_TOKEN_URL_OVERRIDE")}
    env.update(CODEX_HOME=str(home), CODEX_CA_CERTIFICATE=str(certificate), NO_PROXY="*")
    process = subprocess.Popen([str(program), "serve", "--codex-bin", str(wrapper),
        "--state-dir", str(home / "agent-state"), "--listen", "127.0.0.1:" + str(port),
        "--key-file", str(key), "--models", "test,reject,slow"], env=env,
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    url = "http://127.0.0.1:" + str(port)
    headers = {"Authorization": "Bearer " + key.read_text(), "Content-Type": "application/json"}
    def send(value):
        return opener.open(urllib.request.Request(url + "/v1/responses",
            data=json.dumps(value).encode(), headers=headers), timeout=20)
    try:
        for _ in range(200):
            assert process.poll() is None, "agent startup failed"
            try:
                with opener.open(urllib.request.Request(url + "/health", headers=headers), timeout=1):
                    break
            except (OSError, urllib.error.URLError):
                time.sleep(0.1)
        else:
            raise AssertionError("agent startup timed out")
        with send({**request, "stream": False}) as response:
            value = json.loads(response.read())
            assert response.status == 200 and value["status"] == "completed"
            assert value["usage"]["total_tokens"] == 5
            assert [call["call_id"] for call in value["output"]] == ["call_1", "call_2"]
        with send(request) as response:
            assert response.headers["Content-Type"].startswith("text/event-stream")
            body = response.read()
            assert b"response.completed" in body and b"call_1" in body and b"call_2" in body
        try:
            send({**request, "model": "reject"})
            raise AssertionError("upstream rejection became success")
        except urllib.error.HTTPError as error:
            assert error.code == 429 and json.loads(error.read())["error"]["code"] == "test_limit"
    finally:
        if process.poll() is None:
            process.send_signal(signal.SIGINT)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=10)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codex-bin", type=Path, required=True)
    parser.add_argument("--agent-bin", type=Path)
    args = parser.parse_args()
    captured = []
    discovery_paths = []
    tls_failures = []
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            discovery_paths.append(self.path)
            if self.path.endswith("/accounts/check"):
                payload = json.dumps({"accounts":[{"id":"test-account","plan_type":"plus",
                    "workspace_backend_origin": endpoint.rsplit("/backend-api",1)[0],
                    "account_routing_override":"NO_CONSTRAINT"}]}).encode()
                self.send_response(200)
                self.send_header("Content-Type","application/json")
                self.send_header("Content-Length",str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
            else:
                self.send_error(404)

        def do_POST(self):
            assert self.path == "/backend-api/codex/responses", "unexpected endpoint"
            assert self.headers["ChatGPT-Account-ID"] == "test-account"
            assert self.headers["Authorization"] == "Bearer synthetic-access"
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            captured.append(request)
            model = request["model"]
            if model == "reject":
                payload = b'{"error":{"code":"test_limit"}}'
                self.send_response(429)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
                return
            if model == "slow":
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                try:
                    for _ in range(200):
                        self.wfile.write(b": keepalive\n\n")
                        self.wfile.flush()
                        time.sleep(0.05)
                except (BrokenPipeError, ConnectionResetError):
                    pass
                return
            calls = [{"type": "function_call", "id": "fc" + str(index), "name": name,
                      "call_id": "call_" + str(index), "arguments": json.dumps({"cmd": request["input"][0]["content"][0]["text"]})}
                     for index, name in enumerate(["exec_command", "apply_patch"], 1)]
            response = {"id": "resp-test", "object": "response", "created_at": 1,
                        "model": model, "status": "completed", "output": calls,
                        "usage": {"input_tokens": 3, "output_tokens": 2, "total_tokens": 5}}
            event = {"type": "response.completed", "sequence_number": 1, "response": response}
            payload = ("event: response.completed\ndata: " + json.dumps(event) + "\n\n").encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

    class TLSServer(ThreadingHTTPServer):
        def get_request(self):
            try:
                return super().get_request()
            except ssl.SSLError as error:
                tls_failures.append(error.reason)
                raise
    server = TLSServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    tls_directory = tempfile.TemporaryDirectory(prefix="llmproxy-codex-test-ca-")
    tls = Path(tls_directory.name)
    subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-days", "1", "-nodes",
                    "-keyout", str(tls / "key.pem"), "-out", str(tls / "cert.pem"),
                    "-subj", "/CN=127.0.0.1", "-addext", "subjectAltName=IP:127.0.0.1"],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run(["openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes",
                    "-keyout", str(tls / "server-key.pem"), "-out", str(tls / "server.csr"),
                    "-subj", "/CN=127.0.0.1"],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    (tls / "server.ext").write_text("basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1\n")
    subprocess.run(["openssl", "x509", "-req", "-in", str(tls / "server.csr"),
                    "-CA", str(tls / "cert.pem"), "-CAkey", str(tls / "key.pem"),
                    "-CAcreateserial", "-out", str(tls / "server-cert.pem"), "-days", "1",
                    "-extfile", str(tls / "server.ext")],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(tls / "server-cert.pem", tls / "server-key.pem")
    server.socket = context.wrap_socket(server.socket, server_side=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    endpoint = "https://127.0.0.1:" + str(server.server_port) + "/backend-api"
    try:
        with tempfile.TemporaryDirectory(prefix="llmproxy-codex-runtime-") as directory:
            home = Path(directory)
            home.chmod(0o700)
            claims = {"email": "synthetic@example.invalid", "https://api.openai.com/auth": {
                "chatgpt_account_id": "test-account", "chatgpt_user_id": "test-user", "chatgpt_plan_type": "plus"}}
            token = encode({"alg": "RS256"}) + "." + encode(claims) + ".c3ludGhldGlj"
            credentials = {"auth_mode": "chatgpt", "tokens": {"id_token": token, "access_token": "synthetic-access",
                                      "refresh_token": "synthetic-refresh", "account_id": "test-account"},
                           "last_refresh": "2099-01-01T00:00:00Z"}
            auth = home / "auth.json"
            auth.write_text(json.dumps(credentials))
            auth.chmod(0o600)
            executed = home / "tool-executed"
            (home / "config.toml").write_text('[mcp_servers.must_not_run]\ncommand="/bin/sh"\nargs=["-c","touch ' + str(executed) + '"]\n')
            marker = "CONVERSATION-MUST-NOT-PERSIST-" + os.urandom(16).hex()
            request = {"model": "test", "instructions": "", "store": False, "stream": True,
                       "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": marker}]}],
                       "tools": [{"type": "function", "name": name, "parameters": {"type": "object"}}
                                 for name in ["exec_command", "apply_patch"]]}
            rpc = RPC(args.codex_bin.resolve(), home, endpoint, tls / "cert.pem")
            try:
                for id, method, params in [(3, "thread/start", {"ephemeral": True}),
                                           (4, "command/exec", {"command": ["touch", str(executed)], "cwd": str(home)})]:
                    rpc.send(id, method, params)
                    assert rpc.response(id)["error"]["code"] == -32000, "execution RPC not blocked"
                rpc.send(5, "llmproxy/responses", {"request": request})
                head = rpc.response(5)
                assert head.get("result", {}).get("status") == 200, "native request failed: " + json.dumps(head) + "; captured=" + str(len(captured)) + "; discovery=" + json.dumps(discovery_paths) + "; tls=" + json.dumps(tls_failures)
                body = rpc.body(5)
                event = json.loads(body.decode().split("data: ", 1)[1])
                assert [call["call_id"] for call in event["response"]["output"]] == ["call_1", "call_2"]
                assert event["response"]["usage"]["total_tokens"] == 5
                second = json.loads(json.dumps(request))
                second["input"].extend(event["response"]["output"])
                second["input"].extend({"type": "function_call_output", "call_id": "call_" + str(index), "output": "client result"}
                                       for index in [1, 2])
                rpc.send(6, "llmproxy/responses", {"request": second})
                assert rpc.response(6)["result"]["status"] == 200
                rpc.body(6)
                assert captured[0] == request and captured[1] == second, "history was changed"
                rejected = {**request, "tools": [{"type": "web_search"}]}
                rpc.send(7, "llmproxy/responses", {"request": rejected})
                assert "error" in rpc.response(7) and len(captured) == 2
                rpc.send(8, "llmproxy/responses", {"request": {**request, "model": "reject"}})
                assert rpc.response(8)["result"]["status"] == 429
                assert json.loads(rpc.body(8))["error"]["code"] == "test_limit"
                rpc.send(9, "llmproxy/responses", {"request": {**request, "model": "slow"}})
                assert rpc.response(9)["result"]["status"] == 200
                rpc.send(10, "llmproxy/cancel", {"requestId": 9})
                ended = ack = False
                while not (ended and ack):
                    value = rpc.read()
                    if value.get("id") == 10:
                        assert "result" in value
                        ack = True
                    elif value.get("method") == "llmproxy/chunk":
                        assert value["params"]["requestId"] == 9
                        if value["params"]["end"]:
                            assert value["params"]["error"]
                            ended = True
            finally:
                rpc.close()
            for crash in [False, True]:
                rpc = RPC(args.codex_bin.resolve(), home, endpoint, tls / "cert.pem")
                try:
                    rpc.send(3, "llmproxy/responses", {"request": {**request, "model": "slow"}})
                    assert rpc.response(3)["result"]["status"] == 200
                finally:
                    rpc.close(crash=crash)
            if args.agent_bin:
                verify_agent(args.agent_bin.resolve(), args.codex_bin.resolve(), home, endpoint, tls / "cert.pem", request)
                assert captured[-3] == request and captured[-2] == request
            assert not executed.exists(), "tool or configured MCP executed"
            for path in home.rglob("*"):
                if path.is_file():
                    assert marker.encode() not in path.read_bytes(), "conversation persisted: " + str(path.relative_to(home))
            assert not (home / "sessions").exists() and not (home / "history.jsonl").exists()
            print("PASS: native auth, exact HTTP/history/usage, client tools, cancellation, EOF and crash; no conversation files" + ("; actual agent HTTP chain" if args.agent_bin else ""))
    finally:
        server.shutdown()
        server.server_close()
        tls_directory.cleanup()


if __name__ == "__main__":
    main()
