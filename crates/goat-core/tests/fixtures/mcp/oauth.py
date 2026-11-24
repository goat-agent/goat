import base64
import hashlib
import json
import sys
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CODES = {}
TOKENS = set()
REFRESHES = set()
PREREGISTERED = ("preregistered", "s3cret")
MODERN = "2026-07-28"
VERSION_KEY = "io.modelcontextprotocol/protocolVersion"


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def base(self):
        return f"http://127.0.0.1:{self.server.server_port}"

    def send(self, status, body=None, headers=()):
        data = b"" if body is None else json.dumps(body).encode()
        self.send_response(status)
        for key, value in headers:
            self.send_header(key, value)
        if body is not None:
            self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def read_body(self):
        return self.rfile.read(int(self.headers.get("content-length", 0))).decode()

    def do_GET(self):
        url = urllib.parse.urlparse(self.path)
        query = dict(urllib.parse.parse_qsl(url.query))
        if url.path == "/.well-known/oauth-protected-resource/mcp":
            return self.send(200, {"resource": f"{self.base()}/mcp", "authorization_servers": [self.base()]})
        if url.path == "/.well-known/oauth-authorization-server":
            return self.send(200, {
                "issuer": self.base(),
                "authorization_endpoint": f"{self.base()}/authorize",
                "token_endpoint": f"{self.base()}/token",
                "registration_endpoint": f"{self.base()}/register",
                "response_types_supported": ["code"],
                "code_challenge_methods_supported": ["S256"],
                "authorization_response_iss_parameter_supported": True,
                "grant_types_supported": ["authorization_code", "refresh_token"],
                "token_endpoint_auth_methods_supported": ["none", "client_secret_basic", "client_secret_post"],
            })
        if url.path == "/authorize":
            if query.get("resource") != f"{self.base()}/mcp" or query.get("code_challenge_method") != "S256":
                return self.send(400, {"error": "invalid_request"})
            code = f"code{len(CODES)}"
            CODES[code] = query["code_challenge"]
            location = query["redirect_uri"] + "?" + urllib.parse.urlencode({"code": code, "state": query["state"], "iss": self.base()})
            return self.send(302, None, [("location", location)])
        self.send(404, {})

    def do_POST(self):
        url = urllib.parse.urlparse(self.path)
        if url.path == "/register":
            request = json.loads(self.read_body())
            return self.send(201, {
                "client_id": "registered",
                "redirect_uris": request["redirect_uris"],
                "token_endpoint_auth_method": "none",
            })
        if url.path == "/token":
            form = dict(urllib.parse.parse_qsl(self.read_body()))
            client, secret = form.get("client_id"), form.get("client_secret")
            basic = self.headers.get("authorization", "")
            if basic.startswith("Basic "):
                pair = base64.b64decode(basic[6:]).decode()
                client, secret = (urllib.parse.unquote_plus(part) for part in pair.split(":", 1))
            if client == PREREGISTERED[0] and secret != PREREGISTERED[1]:
                return self.send(401, {"error": "invalid_client"})
            if form.get("grant_type") == "refresh_token":
                if form.get("refresh_token") not in REFRESHES:
                    return self.send(400, {"error": "invalid_grant"})
            else:
                challenge = CODES.pop(form.get("code", ""), None)
                verifier = form.get("code_verifier", "").encode()
                expected = base64.urlsafe_b64encode(hashlib.sha256(verifier).digest()).rstrip(b"=").decode()
                if challenge != expected:
                    return self.send(400, {"error": "invalid_grant"})
            number = len(TOKENS) + 1
            TOKENS.add(f"access{number}")
            REFRESHES.add(f"refresh{number}")
            lifetime = 2 if client == PREREGISTERED[0] else 3600
            return self.send(200, {
                "access_token": f"access{number}",
                "token_type": "Bearer",
                "expires_in": lifetime,
                "refresh_token": f"refresh{number}",
            })
        if url.path == "/mcp":
            token = self.headers.get("authorization", "").removeprefix("Bearer ")
            message = json.loads(self.read_body())
            if token not in TOKENS:
                challenge = f'Bearer resource_metadata="{self.base()}/.well-known/oauth-protected-resource/mcp"'
                return self.send(401, {"error": "unauthorized"}, [("www-authenticate", challenge)])
            if "id" not in message:
                return self.send(202)
            method = message["method"]
            version = message.get("params", {}).get("_meta", {}).get(VERSION_KEY)
            if method == "server/discover":
                result = {
                    "supportedVersions": ["2025-11-25", MODERN],
                    "capabilities": {"tools": {}},
                    "ttlMs": 0,
                    "cacheScope": "private",
                    "_meta": {"io.modelcontextprotocol/serverInfo": {"name": "notes", "version": "0"}},
                }
            elif method == "initialize" and message["params"]["protocolVersion"] >= MODERN:
                return self.send(200, {"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "Method not supported"}})
            elif method == "initialize":
                result = {
                    "protocolVersion": message["params"]["protocolVersion"],
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "notes", "version": "0"},
                }
            elif method == "tools/list":
                result = {"tools": [{"name": "whoami", "description": "Name the token", "inputSchema": {"type": "object"}}]}
            elif method == "tools/call":
                result = {"content": [{"type": "text", "text": f"token {token} over {version}"}]}
            else:
                result = {}
            if version:
                result["resultType"] = "complete"
            return self.send(200, {"jsonrpc": "2.0", "id": message["id"], "result": result})
        self.send(404, {})


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_port, flush=True)
server.serve_forever()
