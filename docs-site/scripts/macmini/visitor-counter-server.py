#!/usr/bin/env python3
"""docs-site 방문자 카운터. 포트 3002에서 리슨하며 Cloudflare Tunnel의
'/api/*' 경로가 이 서버로 라우팅되도록 설정한다 (같은 호스트명, 경로만
분기 — 새 서브도메인/CORS 불필요).

POST /api/visit  body: {"visitor_id": "<client-generated uuid>"}
  그 visitor_id가 오늘 처음이면 today 카운트, 역대 처음이면 total 카운트에
  추가하고 현재 값을 반환한다. 이미 카운트된 id면 증가 없이 현재 값만 반환.
GET  /api/visit
  증가 없이 현재 {today, total}만 반환.

공개 인터넷에 노출되는 엔드포인트라 다음을 강제한다:
  - visitor_id는 UUID 형식만 수용 (임의 문자열로 저장소를 부풀리는 공격 차단)
  - 요청 본문 1KB 제한, Content-Length 파싱 실패는 400
  - today/total 고유 ID 수에 상한 — 초과 시 카운터가 포화될 뿐 무한 성장하지 않음
  - 상태는 메모리에 유지(set, O(1) 조회)하고 변경 시에만 원자적으로 저장
"""
import json
import os
import re
import threading
from datetime import datetime, timezone, timedelta
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

STATS_PATH = "/Users/server/apps/visitor-counter/stats.json"
PORT = 3002
KST = timezone(timedelta(hours=9))

MAX_BODY = 1024
# 무작위 UUID를 대량 생성하는 공격에 대한 최후 방어선 — 정상 트래픽으로는
# 도달할 수 없는 값이고, 도달해도 서비스는 죽지 않고 숫자만 멈춘다.
MAX_TODAY = 100_000
MAX_TOTAL = 2_000_000

UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
)

lock = threading.Lock()


def today_str():
    return datetime.now(KST).date().isoformat()


def load_stats():
    try:
        with open(STATS_PATH, encoding="utf-8") as f:
            data = json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        data = {}
    return {
        "date": data.get("date") or today_str(),
        "today_visitors": set(data.get("today_visitors") or []),
        "total_visitors": set(data.get("total_visitors") or []),
    }


def save_stats(data):
    # 임시 파일에 쓴 뒤 os.replace — 쓰는 도중 죽어도 기존 파일이 깨지지 않음
    tmp = STATS_PATH + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(
            {
                "date": data["date"],
                "today_visitors": sorted(data["today_visitors"]),
                "total_visitors": sorted(data["total_visitors"]),
            },
            f,
            ensure_ascii=False,
        )
    os.replace(tmp, STATS_PATH)


# 시작 시 1회 로드 후 메모리에서 운영 (요청마다 디스크를 읽지 않음)
stats = load_stats()


def rollover_if_needed():
    # lock을 쥔 상태에서 호출
    if stats["date"] != today_str():
        stats["date"] = today_str()
        stats["today_visitors"] = set()
        save_stats(stats)


def counts():
    return {"today": len(stats["today_visitors"]), "total": len(stats["total_visitors"])}


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
            rollover_if_needed()
            result = counts()
        self._send_json(result)

    def do_POST(self):
        if self.path.rstrip("/") != "/api/visit":
            self._send_json({"error": "not found"}, 404)
            return

        try:
            length = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            self._send_json({"error": "bad request"}, 400)
            return
        if length < 0 or length > MAX_BODY:
            self._send_json({"error": "payload too large"}, 413)
            return

        raw = self.rfile.read(length) if length else b"{}"
        try:
            payload = json.loads(raw)
        except json.JSONDecodeError:
            payload = {}
        visitor_id = str(payload.get("visitor_id") or "").strip().lower()
        if not UUID_RE.fullmatch(visitor_id):
            visitor_id = ""  # 형식이 아니면 카운트하지 않고 현재 값만 반환

        with lock:
            rollover_if_needed()
            if visitor_id:
                changed = False
                if (
                    visitor_id not in stats["today_visitors"]
                    and len(stats["today_visitors"]) < MAX_TODAY
                ):
                    stats["today_visitors"].add(visitor_id)
                    changed = True
                if (
                    visitor_id not in stats["total_visitors"]
                    and len(stats["total_visitors"]) < MAX_TOTAL
                ):
                    stats["total_visitors"].add(visitor_id)
                    changed = True
                if changed:
                    save_stats(stats)
            result = counts()

        self._send_json(result)

    def log_message(self, fmt, *args):
        pass  # 조용히 — launchd 로그가 요청마다 커지는 것 방지


if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print(f"visitor-counter-server listening on 127.0.0.1:{PORT}")
    server.serve_forever()
