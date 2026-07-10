# Powder 문서 사이트 (fumadocs)

사용자용 문서 — [fumadocs](https://fumadocs.dev) + Next.js 정적 사이트.

```bash
npm install
npm run dev      # http://localhost:3000/docs
npm run build    # 정적 사이트 -> out/
npm run start    # out/ 서빙
```

문의 폼(고객센터)은 [Web3Forms](https://web3forms.com)로 이메일을 보낸다.
로컬 개발 시 `.env.local`에 `NEXT_PUBLIC_WEB3FORMS_ACCESS_KEY`를 설정해야
폼이 동작한다 (`.env.local`은 git에 커밋되지 않음).

콘텐츠는 `content/docs/*.mdx` (한국어). 페이지 구성은 `meta.json`.

주의: 이 저장소가 네트워크 드라이브에 있으면 Turbopack이 UNC 경로를
해석하지 못해 빌드가 깨집니다 — 스크립트가 `--webpack`을 쓰는 이유입니다.

