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
| `visitor-counter-server.py` | `/Users/server/apps/` | 고유 방문자 카운터(포트 3002), `/api/visit` GET·POST (이번에 추가). 공개 노출이라 보안 강화됨(2026-07-11): visitor_id는 UUID 형식만 수용, 본문 1KB 제한, today 10만/total 200만 상한(초과 시 포화), 상태는 메모리 set으로 유지·변경 시에만 원자적 저장(tmp+rename) |
| `com.powder.visitor-counter.plist` | `~/Library/LaunchAgents/` | 위 서버를 로그인 시 자동 실행 (이번에 추가) |
| `bench-publish.py` | `/Users/server/apps/` | DGX Spark가 SSH로 호출 — `bench-results.json`을 읽어 performance.mdx 4개 로케일의 TS/JS·Python 행과 "마지막 자동 측정" 줄만 갱신 (이번에 추가) |

## 벤치마크 자동화 (DGX Spark → 맥미니)

한 달에 한 번 DGX Spark가 bench-site의 5개 언어 탭(TS/JS·Python·Rust·
Java·Go) 벤치마크를 전부 재실행해 `performance.mdx` 숫자를 자동
갱신한다 (2026-07-11부로 5개 언어 전체 자동화 완료 — Go/JDK 툴체인을
DGX에 설치하고 rust-bench/go-bench/java-bench를 빌드함). 자세한 흐름은
`../dgx/README.md` 참고.

- DGX → 맥미니 결과 전송: `scp` (`~/.ssh/id_ed25519_macmini`, DGX 전용
  키 — 위 loopback 키와는 다른 파일이며 맥미니 `authorized_keys`에 별도
  등록됨)
- 맥미니 쪽 수신 파일: `/Users/server/apps/bench-results.json` (매번 덮어씀,
  기록용 아님)
- 갱신 대상: `/Volumes/Ncode/oss-Ncode/docs-site/content/docs/performance{,.en,.zh,.ja}.mdx`
  (NAS 마운트 = Windows P: 드라이브와 동일 파일) — 정규식으로 5개 언어 행
  숫자만 치환, 나머지 본문은 그대로 둠
- "마지막 자동 측정" 줄은 정적 표 아래가 아니라 상단 `<BenchmarkExplorer
  />` 막대그래프 바로 밑에 삽입됨 (표 아래에 두면 눈에 덜 띈다는 피드백
  반영, 2026-07-11)
- 마커는 MDX가 허용하는 `{/* bench:auto */}` 형식만 써야 함 — HTML 주석
  `<!-- -->`를 썼다가 `next build`가 "Unexpected character `!`"로 실패한
  적 있음 (fumadocs-mdx가 MDX/JSX로 파싱하기 때문)
- 멱등: 마커로 "마지막 자동 측정" 줄을 찾아 교체 — 재실행해도 줄이
  중복되지 않음

## 방문자 카운터 라우팅 (Cloudflare)

`docs.powder-orm.info`의 Published application routes에 경로 규칙이 하나
추가되어 있다 (Cloudflare Zero Trust → Networks → Connectors → 해당
커넥터 → Published application routes):

| # | 호스트명 | Path | Service |
|---|---|---|---|
| 1 | `docs.powder-orm.info` | `api/*` | `http://localhost:3002` |
| 2 | `docs.powder-orm.info` | `*` | `http://localhost:3000` |

path가 있는 규칙이 반드시 catch-all(`*`)보다 **위**에 있어야 매칭된다.
호스트명은 실제 사이트 호스트(`docs.powder-orm.info`)와 정확히 같아야
함 — 처음 설정할 때 `powder-orm.info`(서브도메인 없음)로 잘못 넣어서
한참 안 됐던 적이 있음.

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
