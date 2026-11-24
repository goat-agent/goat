import base64
import json
import sys
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = []
TOKENS = set()


def b64(data):
    return base64.urlsafe_b64encode(data).decode().rstrip("=")


def header(name, value):
    return {"name": name, "value": value}


MESSAGES = {
    "m1": {
        "id": "m1",
        "threadId": "t1",
        "labelIds": ["INBOX", "UNREAD"],
        "snippet": "안녕하세요",
        "internalDate": "1795867200000",
        "historyId": "7",
        "sizeEstimate": 120,
        "payload": {
            "mimeType": "multipart/mixed",
            "headers": [
                header("From", "\"김철수\" <kim@example.com>"),
                header("To", "me@example.com"),
                header("Subject", "회의"),
                header("Message-ID", "<m1@example.com>"),
            ],
            "parts": [
                {
                    "mimeType": "text/plain",
                    "filename": "",
                    "headers": [header("Content-Type", "text/plain; charset=euc-kr")],
                    "body": {"data": b64("안녕하세요, 회의는 3시입니다.".encode("euc-kr"))},
                },
                {
                    "mimeType": "application/pdf",
                    "filename": "안건.pdf",
                    "headers": [],
                    "body": {"attachmentId": "a1", "size": 4},
                },
            ],
        },
    },
    "m2": {
        "id": "m2",
        "threadId": "t1",
        "labelIds": ["INBOX"],
        "snippet": "Thanks",
        "internalDate": "1795870800000",
        "payload": {
            "mimeType": "text/html",
            "headers": [
                header("From", "lee@example.com"),
                header("To", "me@example.com, \"Kim, Chulsoo\" <kim@example.com>"),
                header("Subject", "Re: 회의"),
            ],
            "body": {"data": b64("<p>Thanks <b>all</b></p>".encode())},
        },
    },
    "d1": {
        "id": "d1",
        "threadId": "t1",
        "labelIds": ["DRAFT"],
        "snippet": "draft",
        "payload": {"mimeType": "text/plain", "headers": [header("Subject", "draft")], "body": {"data": b64(b"draft")}},
    },
}
THREADS = {"t1": ["m1", "m2", "d1"]}


def metadata(message):
    shown = dict(message)
    shown["payload"] = {"headers": message["payload"]["headers"]}
    return shown


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

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

    def body(self):
        return self.rfile.read(int(self.headers.get("content-length", 0))).decode()

    def authorized(self):
        token = self.headers.get("authorization", "").removeprefix("Bearer ")
        if token in TOKENS:
            return True
        self.send(401, {"error": {"message": "Request had invalid authentication credentials."}})
        return False

    def do_GET(self):
        url = urllib.parse.urlparse(self.path)
        query = urllib.parse.parse_qs(url.query)
        if url.path == "/authorize":
            location = query["redirect_uri"][0] + "?" + urllib.parse.urlencode({"code": "code1", "state": query["state"][0]})
            LOG.append(("authorize", query["scope"][0]))
            return self.send(302, None, [("location", location)])
        if url.path == "/_log":
            return self.send(200, LOG)
        if not url.path.startswith("/gmail/v1/users/me/"):
            return self.send(404, {})
        if not self.authorized():
            return None
        path = url.path.removeprefix("/gmail/v1/users/me/")
        form = query.get("format", ["full"])[0]
        if path == "threads":
            return self.send(200, {"threads": [{"id": "t1"}], "resultSizeEstimate": 1})
        if path.startswith("threads/"):
            messages = [MESSAGES[m] for m in THREADS[path.split("/")[1]]]
            if form == "metadata":
                messages = [metadata(m) for m in messages]
            return self.send(200, {"id": "t1", "messages": messages})
        if path.startswith("messages/") and "/attachments/" not in path:
            message = MESSAGES[path.split("/")[1]]
            return self.send(200, metadata(message) if form == "metadata" else message)
        if path == "labels":
            return self.send(200, {"labels": [{"id": "INBOX", "name": "INBOX", "type": "system"}, {"id": "Label_1", "name": "Work", "type": "user", "labelListVisibility": "labelShow", "color": {"backgroundColor": "#fb4c2f", "textColor": "#ffffff"}}]})
        if path == "settings/sendAs":
            return self.send(200, {"sendAs": [{"sendAsEmail": "me@example.com", "isDefault": True, "signature": "<b>Mino</b>"}]})
        if path.startswith("drafts/"):
            draft = path.split("/")[1]
            return self.send(200, {"id": draft, "message": metadata(MESSAGES["d1"]) if form == "metadata" else MESSAGES["d1"]})
        if path == "drafts":
            return self.send(200, {"drafts": [{"id": "r1"}]})
        self.send(404, {"error": {"message": "no such path " + path}})

    def do_POST(self):
        url = urllib.parse.urlparse(self.path)
        if url.path == "/token":
            form = dict(urllib.parse.parse_qsl(self.body()))
            if form.get("client_secret", "") == "":
                return self.send(401, {"error": "invalid_client"})
            if form["grant_type"] == "refresh_token":
                if form.get("refresh_token") != "refresh1":
                    return self.send(400, {"error": "invalid_grant"})
                LOG.append(("refresh", ""))
            elif form.get("code") != "code1" or not form.get("code_verifier"):
                return self.send(400, {"error": "invalid_grant"})
            token = f"access{len(TOKENS) + 1}"
            TOKENS.add(token)
            reply = {"access_token": token, "expires_in": 60, "token_type": "Bearer"}
            if form["grant_type"] == "authorization_code":
                reply["refresh_token"] = "refresh1"
            return self.send(200, reply)
        if not url.path.startswith("/gmail/v1/users/me/"):
            return self.send(404, {})
        if not self.authorized():
            return None
        path = url.path.removeprefix("/gmail/v1/users/me/")
        body = json.loads(self.body() or "{}")
        LOG.append((path, body))
        if path == "drafts":
            raw = base64.urlsafe_b64decode(body["message"]["raw"] + "==").decode()
            LOG.append(("draft raw", raw))
            return self.send(200, {"id": "r1", "message": {"id": "d1", "threadId": body["message"].get("threadId", "new")}})
        if path.endswith("/modify"):
            return self.send(200, {"id": path.split("/")[1], "labelIds": body.get("addLabelIds", [])})
        if path.endswith("/trash") or path.endswith("/untrash"):
            return self.send(200, {"id": path.split("/")[1]})
        if path == "labels":
            return self.send(200, {"id": "Label_9", "name": body["name"], "type": "user", "color": body.get("color")})
        self.send(404, {"error": {"message": "no such path " + path}})


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_port, flush=True)
server.serve_forever()
