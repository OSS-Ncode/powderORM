# Powder 문서 사이트 (fumadocs)

사용자용 문서 — [fumadocs](https://fumadocs.dev) + Next.js 정적 사이트.

```bash
npm install
npm run dev      # http://localhost:3000/docs
npm run build    # 정적 사이트 -> out/
npm run start    # out/ 서빙
```

콘텐츠는 `content/docs/*.mdx` (한국어). 페이지 구성은 `meta.json`.

주의: 이 저장소가 네트워크 드라이브에 있으면 Turbopack이 UNC 경로를
해석하지 못해 빌드가 깨집니다 — 스크립트가 `--webpack`을 쓰는 이유입니다.
// test comment 2026년 07월 10일 금 오전 11:06:04
// watch-test 2026년 07월 10일 금 오전 11:06:19
<!-- sync test 2026년 07월 10일 금 오전 11:32:15 -->
<!-- real change test 2026년 07월 10일 금 오전 11:34:15 -->
