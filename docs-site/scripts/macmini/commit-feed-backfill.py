#!/usr/bin/env python3
"""GitHub API로 powderORM main의 최근 50개 커밋을 가져와 feed.json을
1회성으로 채운다. 맥미니에서 수동으로 한 번만 실행한다.
"""
import json
import sys
import urllib.error
import urllib.request

OLLAMA_URL = "http://100.100.103.17:11434/api/generate"
MODEL = "qwen3:8b"
FEED_PATH = "/Users/server/apps/commit-feed/feed.json"
REPO = "OSS-Ncode/powderORM"
COUNT = 50
DIFF_TRUNCATE = 4000

# 지시문 자체를 대상 언어로 써야 작은 모델이 그 언어로 답한다 — 한국어
# 지시문에 "영어로 답해"만 끼워넣으면 한국어로 답하는 경우가 많았음
# (qwen3:8b 실측 확인됨, commit-feed-append.py와 동일한 수정).
PROMPTS = {
    "ko": (
        "다음은 git 커밋 메시지와 diff입니다. 한국어로 2~3문장으로 무엇이 "
        "왜 바뀌었는지 자연어로 요약해줘. 코드를 그대로 인용하지 말고, "
        "반드시 한국어로만 답해. 한자(漢字)나 중국어를 섞지 말고 순한글로만 써."
    ),
    "en": (
        "Below is a git commit message and diff. Summarize in 2-3 sentences, "
        "in natural language, what changed and why. Do not quote code "
        "verbatim. Respond only in English."
    ),
    "zh": (
        "以下是 git 提交信息和 diff。请用 2-3 句话，以自然语言概括发生了"
        "什么变化以及原因，不要直接引用代码。请只用简体中文回答。"
    ),
    "ja": (
        "以下は git のコミットメッセージと diff です。2〜3文の自然な文章で、"
        "何がなぜ変更されたのかを要約してください。コードをそのまま引用"
        "しないでください。必ず日本語のみで答えてください。"
    ),
}
LANG_NAMES = {"ko": "한국어", "en": "English", "zh": "简体中文", "ja": "日本語"}


def gh_get(url, accept):
    req = urllib.request.Request(url, headers={"Accept": accept, "User-Agent": "powder-commit-feed"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        return resp.read()


def summarize(lang, message, diff):
    prompt = (
        f"{PROMPTS[lang]}\n\n"
        f"commit message:\n{message}\n\ndiff:\n{diff[:DIFF_TRUNCATE]}"
    )
    payload = json.dumps({"model": MODEL, "prompt": prompt, "stream": False}).encode("utf-8")
    req = urllib.request.Request(OLLAMA_URL, data=payload, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            return (data.get("response") or "").strip() or None
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, OSError) as e:
        print(f"[{lang}] summarize failed: {e}", file=sys.stderr)
        return None


def main():
    commits = json.loads(gh_get(
        f"https://api.github.com/repos/{REPO}/commits?sha=main&per_page={COUNT}",
        "application/vnd.github+json",
    ))

    feed = []
    for c in reversed(commits):  # 오래된 것부터 append해서 최신이 배열 끝에 오게
        sha = c["sha"]
        short_sha = sha[:7]
        author = c["commit"]["author"]["name"]
        timestamp = c["commit"]["author"]["date"]
        message = c["commit"]["message"]
        url = c["html_url"]

        try:
            diff = gh_get(
                f"https://api.github.com/repos/{REPO}/commits/{sha}",
                "application/vnd.github.v3.diff",
            ).decode("utf-8", errors="replace")
        except (urllib.error.URLError, OSError) as e:
            print(f"diff fetch failed for {short_sha}: {e}", file=sys.stderr)
            diff = ""

        summary = {lang: summarize(lang, message, diff) for lang in LANG_NAMES}

        feed.append({
            "sha": sha,
            "short_sha": short_sha,
            "author": author,
            "message": message.strip(),
            "summary": summary,
            "timestamp": timestamp,
            "url": url,
        })
        print(f"backfilled {short_sha}")

    with open(FEED_PATH, "w", encoding="utf-8") as f:
        json.dump(feed, f, ensure_ascii=False, indent=2)

    print(f"wrote {len(feed)} entries to {FEED_PATH}")


if __name__ == "__main__":
    main()
