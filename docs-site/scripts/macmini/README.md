# 맥미니 docs-site 자동 배포 구조

Windows(P: 드라이브)에서 docs-site 파일을 수정하면 맥미니가 자동으로
동기화 → 빌드 → 서버 재시작까지 수행한다. 이 폴더는 맥미니에 배포된
파일들의 사본(기록용)이다.

## 전체 흐름

```
Windows (P:\oss-Ncode) ──저장──▶ NAS (\\100.123.184.127\Ncode)
                                     │ SMB (/Volumes/Ncode, 맥미니가 마운트)
                                     ▼
① docs-site-sync.sh (LaunchAgent, 5초 주기 rsync)
     /Volumes/Ncode/oss-Ncode/docs-site → /Users/server/apps/docs-site
                                     ▼
② docs-site-watch.sh (root LaunchDaemon, 기존 스크립트)
     로컬 복사본 내용 해시 변경 감지 → npm run build → launchctl kickstart
                                     ▼
③ serve out (-l 3000) ← cloudflared 터널이 도메인으로 노출
```

수정 후 사이트 반영까지 보통 15~30초 (빌드 시간 포함).

## 맥미니에 배포된 파일

| 파일 | 맥미니 위치 | 역할 |
|---|---|---|
| `docs-site-sync.sh` | `/Users/server/apps/` | NAS → 로컬 rsync 루프 (이번에 추가) |
| `com.powder.docs-site-sync.plist` | `~/Library/LaunchAgents/` | 위 스크립트를 로그인 시 자동 실행 |
| `docs-site-watch.sh` | `/Users/server/apps/` | 로컬 변경 감지 → 빌드 → commit-feed/feed.json을 out/commit-feed.json으로 복사(cp -f) → 재시작 (이번에 복사 스텝 추가 — 처음엔 심볼릭 링크였으나 serve가 심볼릭 링크 서빙을 거부해서 cp로 변경, 기록 사본도 이번에 추가) |
| (기존) `com.powder.docs-site*.plist` | `/Library/LaunchDaemons/` | serve 데몬 + watch 데몬 |
| `commit-feed-append.py` | `/Users/server/apps/` | 커밋 1개를 받아 Ollama로 언어별 요약 후 feed.json에 append (이번에 추가) |
| `commit-feed-backfill.py` | `/Users/server/apps/` | 최초 1회 실행용 — 최근 50개 커밋 백필 (이번에 추가) |

## 왜 SSH loopback으로 실행하나

`com.powder.docs-site-sync.plist`는 스크립트를 `ssh server@127.0.0.1`을
경유해 실행한다. macOS TCC 정책상 launchd가 직접 띄운 프로세스는
네트워크 볼륨(SMB) 접근이 거부되지만, sshd 세션은 허용되어 있기 때문.
loopback 인증 키는 `~/.ssh/id_ed25519_local`.

NAS 마운트가 끊기면 스크립트가 키체인 자격증명으로 자동 재마운트한다
(`osascript mount volume`). 재부팅 후에도 자동 복구됨.

## 관리 (맥미니에서, 또는 `ssh macmini-docs`로)

```bash
tail -f ~/apps/docs-site-sync.log     # 동기화 로그
tail -f ~/apps/docs-site-watch.log    # 빌드/재시작 로그
launchctl kickstart -k gui/$(id -u)/com.powder.docs-site-sync   # 동기화 재시작
launchctl bootout gui/$(id -u)/com.powder.docs-site-sync        # 동기화 중지
```

Windows 쪽 SSH 별칭: `~/.ssh/config`의 `macmini-docs` (키 인증).

## 스크립트 수정 시

이 폴더의 파일을 고친 뒤 맥미니로 다시 배포해야 적용된다:

```bash
scp docs-site-sync.sh macmini-docs:/Users/server/apps/docs-site-sync.sh
ssh macmini-docs "launchctl kickstart -k gui/\$(id -u)/com.powder.docs-site-sync"
```

(이 폴더 `scripts/macmini`는 rsync 대상에서 제외되어 있어 맥미니 빌드에는
영향을 주지 않는다.)
