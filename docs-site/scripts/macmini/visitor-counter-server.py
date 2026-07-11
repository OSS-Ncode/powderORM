#!/usr/bin/env python3
"""docs-site 방문자 카운터. 포트 3002에서 리슨하며 Cloudflare Tunnel의
'/api/*' 경로가 이 서버로 라우팅되도록 설정한다 (같은 호스트명, 경로만
분기 — 새 서브도메인/CORS 불필요).

POST /api/visit  body: {"visitor_id": "<client-generated uuid>"}
  그 visitor_id가 오늘 처음이면 today 카운트, 역대 처음이면 total 카운트에
  추가하고 현재 값을 반환한다. 이미 카운트된 id면 증가 없이 현재 값만 반환.
GET  /api/visit
  증가 없이 현재 {today, total}만 반환.
"""
import json
import threading
from datetime import date, datetime, timezone, timedelta
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

STATS_PATH = "/Users/server/apps/visitor-counter/stats.json"
PORT = 3002
KST = timezone(timedelta(hours=9))

lock = threading.Lock()


def today_str():
    return datetime.now(KST).date().isoformat()


def load_stats():
    try:
        with open(STATS_PATH, encoding="utf-8") as f:
            data = json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        data = {"date": today_str(), "today_visitors": [], "total_visitors": []}

    if data.get("date") != today_str():
        data["date"] = today_str()
        data["today_visitors"] = []

    return data


def save_stats(data):
    with open(STATS_PATH, "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, indent=2)


def counts(data):
    return {"today": len(data["today_visitors"]), "total": len(data["total_visitors"])}


class Handler(BaseHTTPRequestHandler):
    def _send_json(self, obj, status=200):
        body = json.dumps(obj).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_OPTIONS(self):
        self.send_response(204)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type")
        self.end_headers()

    def do_GET(self):
        if self.path.rstrip("/") != "/api/visit":
            self._send_json({"error": "not found"}, 404)
            return
        with lock:
            data = load_stats()
        self._send_json(counts(data))

    def do_POST(self):
        if self.path.rstrip("/") != "/api/visit":
            self._send_json({"error": "not found"}, 404)
            return

        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length) if length else b"{}"
        try:
            payload = json.loads(raw)
        except json.JSONDecodeError:
            payload = {}
        visitor_id = str(payload.get("visitor_id") or "").strip()

        with lock:
            data = load_stats()
            if visitor_id:
                if visitor_id not in data["today_visitors"]:
                    data["today_visitors"].append(visitor_id)
                if visitor_id not in data["total_visitors"]:
                    data["total_visitors"].append(visitor_id)
                save_stats(data)
            result = counts(data)

        self._send_json(result)

    def log_message(self, fmt, *args):
        pass  # 조용히 — launchd 로그가 요청마다 커지는 것 방지


if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print(f"visitor-counter-server listening on 127.0.0.1:{PORT}")
    server.serve_forever()
