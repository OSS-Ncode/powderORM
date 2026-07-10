#!/usr/bin/env bash
# 로컬 docs-site 소스를 macmini-docs(맥미니)로 그대로 동기화만 한다 (빌드/재시작은 하지 않음).
# 빌드와 서버 재시작은 맥미니에서 돌고 있는 docs-site-watch 데몬이 변경을 감지해 알아서 처리한다.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOCS_DIR="$REPO_ROOT/docs-site"
REMOTE_HOST="macmini-docs"
REMOTE_APP_DIR="apps/docs-site"
TMP_ARCHIVE="$(mktemp -u /tmp/docs-site-sync-XXXXXX).tar.gz"

if [ ! -d "$DOCS_DIR" ]; then
  echo "docs-site 디렉터리를 찾을 수 없습니다: $DOCS_DIR" >&2
  exit 1
fi

tar -C "$REPO_ROOT" \
  --exclude 'docs-site/node_modules' \
  --exclude 'docs-site/.next' \
  --exclude 'docs-site/.source' \
  --exclude 'docs-site/out' \
  -czf "$TMP_ARCHIVE" docs-site

scp -q "$TMP_ARCHIVE" "$REMOTE_HOST:/tmp/docs-site-sync.tar.gz"
rm -f "$TMP_ARCHIVE"

ssh "$REMOTE_HOST" "
  set -e
  mkdir -p ~/$REMOTE_APP_DIR
  cd ~/$REMOTE_APP_DIR
  # node_modules/.next/.source/out(빌드 캐시)를 제외한 소스 파일을 전부 지우고 새로 푼다.
  find . -mindepth 1 -maxdepth 1 \
    ! -name node_modules ! -name .next ! -name .source ! -name out \
    -exec rm -rf {} +
  tar xzf /tmp/docs-site-sync.tar.gz -C ~/$REMOTE_APP_DIR --strip-components=1
  rm -f /tmp/docs-site-sync.tar.gz
"

echo "[sync-docs] 동기화 완료 $(date +%H:%M:%S)"
