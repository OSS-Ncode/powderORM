#!/usr/bin/env python3
"""GitHub Actions에서 커밋 하나를 받아 DGX Spark Ollama로 언어별 요약을
만들고 /Users/server/apps/commit-feed/feed.json에 추가한다.

사용법:
  commit-feed-append.py <sha> <short_sha> <author> <url> <timestamp> \
      <message_file> <diff_file>

요약 실패는 non-fatal — 실패한 언어만 null로 기록하고 계속 진행한다.
"""
import json
import sys
import urllib.error
import urllib.request

OLLAMA_URL = "http://100.100.103.17:11434/api/generate"
MODEL = "qwen3:8b"
FEED_PATH = "/Users/server/apps/commit-feed/feed.json"
MAX_ENTRIES = 50
DIFF_TRUNCATE = 4000

LANG_NAMES = {
    "ko": "한국어",
    "en": "English",
    "zh": "简体中文",
    "ja": "日本語",
}


def summarize(lang, message, diff):
    prompt = (
        f"다음은 git 커밋 메시지와 diff입니다. {LANG_NAMES[lang]}로 2~3문장으로 "
        f"무엇이 왜 바뀌었는지 자연어로 요약해줘. 코드를 그대로 인용하지 마.\n\n"
        f"커밋 메시지:\n{message}\n\ndiff:\n{diff[:DIFF_TRUNCATE]}"
    )
    payload = json.dumps(
        {"model": MODEL, "prompt": prompt, "stream": False}
    ).encode("utf-8")
    req = urllib.request.Request(
        OLLAMA_URL, data=payload, headers={"Content-Type": "application/json"}
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            text = (data.get("response") or "").strip()
            return text or None
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, OSError) as e:
        print(f"[{lang}] summarize failed: {e}", file=sys.stderr)
        return None


def main():
    if len(sys.argv) != 8:
        print("usage: commit-feed-append.py <sha> <short_sha> <author> <url> "
              "<timestamp> <message_file> <diff_file>", file=sys.stderr)
        sys.exit(0)

    sha, short_sha, author, url, timestamp, message_file, diff_file = sys.argv[1:8]

    with open(message_file, encoding="utf-8") as f:
        message = f.read()
    with open(diff_file, encoding="utf-8") as f:
        diff = f.read()

    summary = {lang: summarize(lang, message, diff) for lang in LANG_NAMES}

    entry = {
        "sha": sha,
        "short_sha": short_sha,
        "author": author,
        "message": message.strip(),
        "summary": summary,
        "timestamp": timestamp,
        "url": url,
    }

    try:
        with open(FEED_PATH, encoding="utf-8") as f:
            feed = json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        feed = []

    feed = [e for e in feed if e.get("sha") != sha]  # 재실행 시 중복 방지
    feed.append(entry)
    feed = feed[-MAX_ENTRIES:]

    with open(FEED_PATH, "w", encoding="utf-8") as f:
        json.dump(feed, f, ensure_ascii=False, indent=2)

    print(f"appended {short_sha}")


if __name__ == "__main__":
    main()
