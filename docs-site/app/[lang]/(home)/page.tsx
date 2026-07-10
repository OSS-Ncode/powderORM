import Link from 'next/link';
import { ArrowRight, Boxes, Gauge, ShieldCheck, FileCode2, Layers } from 'lucide-react';
import { gitConfig } from '@/lib/shared';

// lucide-react (this version) ships no brand icons, so inline the GitHub mark.
function GithubMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden className={className}>
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0016 8c0-4.42-3.58-8-8-8z" />
    </svg>
  );
}

type Copy = {
  badge: string;
  title: React.ReactNode;
  subtitle: string;
  ctaPrimary: string;
  ctaSecondary: string;
  installNote: string;
  valuesHeading: string;
  values: { icon: React.ReactNode; title: string; body: string }[];
  demoHeading: string;
  demoSub: string;
  benchHeading: string;
  bench: { value: string; label: string }[];
  matrixHeading: string;
  langsLabel: string;
  dbsLabel: string;
};

const GH = `https://github.com/${gitConfig.user}/${gitConfig.repo}`;

const copy: Record<string, Copy> = {
  ko: {
    badge: 'Rust 코어 · 9개 언어 · zero-copy',
    title: (
      <>
        하나의 엔진으로
        <br />
        <span className="text-fd-primary">모든 언어에서</span> 빠른 데이터 접근
      </>
    ),
    subtitle:
      'Powder는 Rust 코어 하나에 9개 언어 바인딩을 얹은 ORM입니다. 쿼리 결과를 컬럼 버퍼(PCB)로 한 번에 넘겨 언어 경계의 변환 비용을 없앴어요. 스키마 파일 하나에서 typed 모델·마이그레이션·검증이 나옵니다.',
    ctaPrimary: '5분 만에 시작하기',
    ctaSecondary: 'GitHub',
    installNote: '설치는 언어별 패키지 매니저 한 줄이면 끝나요.',
    valuesHeading: '왜 Powder를 쓰나요',
    values: [
      {
        icon: <Gauge className="size-5" />,
        title: '진짜로 빠릅니다',
        body: '5개 언어 벤치마크 전부에서 그 언어의 raw SQL 드라이버보다 빨라요. 행마다 FFI를 왕복하지 않고 컬럼 버퍼를 한 번만 넘기거든요.',
      },
      {
        icon: <Boxes className="size-5" />,
        title: '하나의 코어, 하나의 버그',
        body: 'TS·Python·Kotlin ORM과 모든 드라이버가 같은 Rust 엔진을 공유합니다. 한 곳을 고치면 모든 언어가 함께 나아져요.',
      },
      {
        icon: <FileCode2 className="size-5" />,
        title: '스키마가 곧 진실',
        body: 'powder.schema.json 하나로 DDL·마이그레이션·드리프트 검증(빌드 게이트)·typed 모델·관계·명명 쿼리가 전부 나옵니다.',
      },
      {
        icon: <ShieldCheck className="size-5" />,
        title: '정직하게 실패합니다',
        body: '매핑 못 하는 타입, 전 행을 지울 뻔한 delete, 잘못된 자리표시자는 조용히 넘어가지 않고 해결책이 담긴 오류를 냅니다.',
      },
    ],
    demoHeading: '30초 훑어보기',
    demoSub: '읽히는 대로 동작하는 문법. N+1 없는 관계 로딩까지.',
    benchHeading: '숫자로 보는 성능',
    bench: [
      { value: '3~4ms', label: '콜드 쿼리 @ 200k행' },
      { value: '~25×', label: 'node:sqlite 대비' },
      { value: '0.02ms', label: '반복 쿼리(캐시)' },
    ],
    matrixHeading: '지원 범위',
    langsLabel: '언어',
    dbsLabel: '데이터베이스',
  },
  en: {
    badge: 'Rust core · 9 languages · zero-copy',
    title: (
      <>
        One engine,
        <br />
        fast data access in <span className="text-fd-primary">every language</span>
      </>
    ),
    subtitle:
      'Powder is an ORM built on a single Rust core with bindings for 9 languages. Query results cross the language boundary once, as a column buffer (PCB) — no per-row conversion tax. One schema file gives you typed models, migrations, and validation.',
    ctaPrimary: 'Get started in 5 min',
    ctaSecondary: 'GitHub',
    installNote: 'Install is a one-liner in your language’s package manager.',
    valuesHeading: 'Why Powder',
    values: [
      {
        icon: <Gauge className="size-5" />,
        title: 'Genuinely fast',
        body: 'Faster than the raw SQL driver in every one of five language benchmarks. Results move as a single column buffer instead of a per-row FFI round trip.',
      },
      {
        icon: <Boxes className="size-5" />,
        title: 'One core, one bug',
        body: 'The TS, Python, and Kotlin ORMs and every driver share the same Rust engine. Fix it once, and every language gets better.',
      },
      {
        icon: <FileCode2 className="size-5" />,
        title: 'Schema is the source of truth',
        body: 'A single powder.schema.json produces DDL, migrations, drift validation (a build gate), typed models, relations, and named queries.',
      },
      {
        icon: <ShieldCheck className="size-5" />,
        title: 'Fails honestly',
        body: 'Unmappable types, a delete about to wipe every row, a bad placeholder — none pass silently. You get an error with the fix in it.',
      },
    ],
    demoHeading: 'A 30-second look',
    demoSub: 'Syntax that does what it reads like — including N+1-free relation loading.',
    benchHeading: 'Performance, in numbers',
    bench: [
      { value: '3–4ms', label: 'cold query @ 200k rows' },
      { value: '~25×', label: 'vs node:sqlite' },
      { value: '0.02ms', label: 'repeat query (cached)' },
    ],
    matrixHeading: 'What’s supported',
    langsLabel: 'Languages',
    dbsLabel: 'Databases',
  },
};

const DEMO = `const db = powder(await Client.connect("app.db"));

// Reads the way it looks
const u   = await db.users.find(1);
const top = await db.users.where("score", ">=", 5)
  .orderBy("score", "desc").limit(10).all();
const page = await db.users.orderBy("id").paginate(1, 20);

// Relations, no N+1 (one batched IN per level)
const posts = await db.posts.findMany({ include: { user: true } });`;

const LANGS = [
  'TypeScript',
  'Python',
  'Rust',
  'Java',
  'Kotlin',
  'Go',
  'C',
  'C++',
  'C#',
];
const DBS = ['SQLite', 'PostgreSQL', 'MySQL / MariaDB', 'Oracle'];

export default async function HomePage({ params }: { params: Promise<{ lang: string }> }) {
  const { lang } = await params;
  const t = copy[lang] ?? copy.ko;

  return (
    <main className="flex flex-1 flex-col">
      {/* Hero */}
      <section className="relative overflow-hidden border-b border-fd-border">
        <div
          className="pointer-events-none absolute inset-0 opacity-60"
          style={{
            background:
              'radial-gradient(60% 60% at 50% 0%, color-mix(in oklab, var(--color-fd-primary) 18%, transparent) 0%, transparent 70%)',
          }}
        />
        <div className="relative mx-auto flex max-w-4xl flex-col items-center px-6 py-24 text-center">
          <span className="mb-6 inline-flex items-center gap-2 rounded-full border border-fd-border bg-fd-card px-4 py-1.5 text-sm text-fd-muted-foreground">
            <Layers className="size-4 text-fd-primary" />
            {t.badge}
          </span>
          <h1 className="text-balance text-4xl font-bold tracking-tight sm:text-5xl md:text-6xl">
            {t.title}
          </h1>
          <p className="mt-6 max-w-2xl text-balance text-lg text-fd-muted-foreground">
            {t.subtitle}
          </p>
          <div className="mt-8 flex flex-wrap items-center justify-center gap-3">
            <Link
              href={`/${lang}/docs/quickstart`}
              className="inline-flex items-center gap-2 rounded-lg bg-fd-primary px-5 py-2.5 font-medium text-fd-primary-foreground transition-opacity hover:opacity-90"
            >
              {t.ctaPrimary}
              <ArrowRight className="size-4" />
            </Link>
            <a
              href={GH}
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center gap-2 rounded-lg border border-fd-border bg-fd-card px-5 py-2.5 font-medium transition-colors hover:bg-fd-accent"
            >
              <GithubMark className="size-4" />
              {t.ctaSecondary}
            </a>
          </div>
          <p className="mt-4 text-sm text-fd-muted-foreground">{t.installNote}</p>
        </div>
      </section>

      {/* Values */}
      <section className="mx-auto w-full max-w-5xl px-6 py-20">
        <h2 className="mb-10 text-center text-2xl font-semibold">{t.valuesHeading}</h2>
        <div className="grid gap-4 sm:grid-cols-2">
          {t.values.map((v) => (
            <div
              key={v.title}
              className="rounded-xl border border-fd-border bg-fd-card p-6 transition-colors hover:border-fd-primary/40"
            >
              <div className="mb-3 inline-flex size-10 items-center justify-center rounded-lg bg-fd-primary/10 text-fd-primary">
                {v.icon}
              </div>
              <h3 className="mb-1.5 font-semibold">{v.title}</h3>
              <p className="text-sm leading-relaxed text-fd-muted-foreground">{v.body}</p>
            </div>
          ))}
        </div>
      </section>

      {/* Code demo */}
      <section className="border-y border-fd-border bg-fd-muted/30">
        <div className="mx-auto grid w-full max-w-5xl items-center gap-8 px-6 py-20 md:grid-cols-2">
          <div>
            <h2 className="text-2xl font-semibold">{t.demoHeading}</h2>
            <p className="mt-3 text-fd-muted-foreground">{t.demoSub}</p>
          </div>
          <div className="overflow-x-auto rounded-xl border border-fd-border bg-[#0b1020] p-5 text-sm shadow-lg">
            <pre className="text-slate-200">
              <code>{DEMO}</code>
            </pre>
          </div>
        </div>
      </section>

      {/* Benchmarks */}
      <section className="mx-auto w-full max-w-5xl px-6 py-20">
        <h2 className="mb-10 text-center text-2xl font-semibold">{t.benchHeading}</h2>
        <div className="grid gap-4 sm:grid-cols-3">
          {t.bench.map((b) => (
            <div
              key={b.label}
              className="rounded-xl border border-fd-border bg-fd-card p-8 text-center"
            >
              <div className="text-4xl font-bold text-fd-primary">{b.value}</div>
              <div className="mt-2 text-sm text-fd-muted-foreground">{b.label}</div>
            </div>
          ))}
        </div>
      </section>

      {/* Support matrix */}
      <section className="border-t border-fd-border">
        <div className="mx-auto w-full max-w-5xl px-6 py-20">
          <h2 className="mb-10 text-center text-2xl font-semibold">{t.matrixHeading}</h2>
          <div className="grid gap-8 md:grid-cols-2">
            <div>
              <div className="mb-3 text-sm font-medium text-fd-muted-foreground">{t.langsLabel}</div>
              <div className="flex flex-wrap gap-2">
                {LANGS.map((l) => (
                  <span
                    key={l}
                    className="rounded-md border border-fd-border bg-fd-card px-3 py-1.5 text-sm"
                  >
                    {l}
                  </span>
                ))}
              </div>
            </div>
            <div>
              <div className="mb-3 text-sm font-medium text-fd-muted-foreground">{t.dbsLabel}</div>
              <div className="flex flex-wrap gap-2">
                {DBS.map((d) => (
                  <span
                    key={d}
                    className="rounded-md border border-fd-border bg-fd-card px-3 py-1.5 text-sm"
                  >
                    {d}
                  </span>
                ))}
              </div>
            </div>
          </div>
        </div>
      </section>
    </main>
  );
}
