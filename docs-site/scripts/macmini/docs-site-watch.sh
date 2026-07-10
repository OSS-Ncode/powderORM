#!/bin/zsh
# docs-site 소스 변경을 감지해 자동으로 npm run build 후 serve 데몬을 재시작한다.
# root(LaunchDaemon)로 실행되므로 HOME에 의존하지 않고 절대 경로를 사용한다.
export PATH="/Users/server/.local/bin:$PATH"

APP_DIR="/Users/server/apps/docs-site"
last_hash=""

# 내용(content) 기준으로 해시를 낸다. next-env.d.ts처럼 빌드할 때마다
# mtime만 갱신되고 내용은 그대로인 파일 때문에 무한 재빌드 루프에 빠지는 것을 방지한다.
hash_sources() {
  find "$APP_DIR" \
    \( -path "$APP_DIR/node_modules" -o -path "$APP_DIR/.next" -o -path "$APP_DIR/.source" -o -path "$APP_DIR/out" \) -prune -o \
    -type f ! -name '*.log' -print0 2>/dev/null \
    | xargs -0 md5 -r 2>/dev/null | sort | md5
}

while true; do
  current_hash="$(hash_sources)"
  if [ "$current_hash" != "$last_hash" ]; then
    if [ -n "$last_hash" ]; then
      echo "[docs-site-watch] $(date '+%H:%M:%S') 변경 감지, 재빌드 시작"
      cd "$APP_DIR"
      if [ package.json -nt node_modules/.deploy-stamp ] 2>/dev/null || [ ! -d node_modules ]; then
        npm install && mkdir -p node_modules && touch node_modules/.deploy-stamp
      fi
      if npm run build; then
        ln -sf /Users/server/apps/commit-feed/feed.json "$APP_DIR/out/commit-feed.json"
        launchctl kickstart -k system/com.powder.docs-site
        echo "[docs-site-watch] $(date '+%H:%M:%S') 빌드 및 재시작 완료"
      else
        echo "[docs-site-watch] $(date '+%H:%M:%S') 빌드 실패, 재시작하지 않음"
      fi
    fi
    last_hash="$current_hash"
  fi
  sleep 3
done
