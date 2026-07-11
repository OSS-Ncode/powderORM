import Link from 'next/link';
import { ArrowRight, Boxes, Gauge, Heart, ShieldCheck, FileCode2, Layers } from 'lucide-react';
import { gitConfig } from '@/lib/shared';
import { CommitFeed } from '@/components/commit-feed';
import { VisitorStats } from '@/components/visitor-stats';
import { SiteFooter } from '@/components/site-footer';

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
  sponsorHeading: string;
  sponsorBody: string;
  sponsorCta: string;
  feedHeading: string;
  feedBody: string;
};

const GH = `https://github.com/${gitConfig.user}/${gitConfig.repo}`;
const SPONSOR_URL = 'https://fairy.hada.io/@ncode-powder-orm';

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
    sponsorHeading: 'Powder가 도움이 됐나요?',
    sponsorBody:
      'Powder는 오픈소스로 개발되고 있어요. 후원은 코어 개발과 더 많은 언어·DB 지원에 큰 힘이 됩니다.',
    sponsorCta: '개발 후원하기',
    feedHeading: '실시간 개발 현황',
    feedBody: 'main 브랜치에 커밋이 올라올 때마다 AI가 요약해서 여기 보여줍니다.',
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
    sponsorHeading: 'Has Powder helped you?',
    sponsorBody:
      'Powder is developed in the open. Sponsorship directly funds core development and broader language & database support.',
    sponsorCta: 'Sponsor the project',
    feedHeading: 'Live development feed',
    feedBody: 'Every commit to main gets summarized by AI and shown here in real time.',
  },
  zh: {
    badge: 'Rust 内核 · 9 种语言 · 零拷贝',
    title: (
      <>
        一个引擎，
        <br />
        让<span className="text-fd-primary">每种语言</span>都能快速访问数据
      </>
    ),
    subtitle:
      'Powder 是一个基于单一 Rust 内核、提供 9 种语言绑定的 ORM。查询结果以列缓冲区（PCB）一次性跨越语言边界，消除了逐行转换的开销。一个模式文件即可生成类型化模型、迁移和校验。',
    ctaPrimary: '5 分钟上手',
    ctaSecondary: 'GitHub',
    installNote: '安装只需在你的包管理器里执行一行命令。',
    valuesHeading: '为什么选择 Powder',
    values: [
      {
        icon: <Gauge className="size-5" />,
        title: '真正的快',
        body: '在全部 5 个语言基准测试中都比该语言的原生 SQL 驱动更快。结果以单个列缓冲区传递，而不是逐行往返 FFI。',
      },
      {
        icon: <Boxes className="size-5" />,
        title: '一个内核，一个 bug',
        body: 'TS、Python、Kotlin 的 ORM 和所有驱动共享同一个 Rust 引擎。修复一处，所有语言一起受益。',
      },
      {
        icon: <FileCode2 className="size-5" />,
        title: '模式即事实来源',
        body: '一个 powder.schema.json 生成 DDL、迁移、漂移校验（构建门禁）、类型化模型、关系和命名查询。',
      },
      {
        icon: <ShieldCheck className="size-5" />,
        title: '诚实地失败',
        body: '无法映射的类型、即将清空整张表的 delete、错误的占位符——都不会被静默忽略，而是抛出带有解决方案的错误。',
      },
    ],
    demoHeading: '30 秒速览',
    demoSub: '代码怎么读就怎么运行的语法，还有无 N+1 的关系加载。',
    benchHeading: '用数字说话',
    bench: [
      { value: '3~4ms', label: '冷查询 @ 20 万行' },
      { value: '~25×', label: '对比 node:sqlite' },
      { value: '0.02ms', label: '重复查询（缓存）' },
    ],
    matrixHeading: '支持范围',
    langsLabel: '语言',
    dbsLabel: '数据库',
    sponsorHeading: 'Powder 帮到你了吗？',
    sponsorBody: 'Powder 是开源项目。你的赞助将直接支持内核开发以及更多语言和数据库的支持。',
    sponsorCta: '赞助这个项目',
    feedHeading: '实时开发动态',
    feedBody: '每次 main 分支有新提交，AI 都会自动摘要并显示在这里。',
  },
  ja: {
    badge: 'Rust コア · 9 言語 · ゼロコピー',
    title: (
      <>
        ひとつのエンジンで
        <br />
        <span className="text-fd-primary">あらゆる言語から</span>高速なデータアクセス
      </>
    ),
    subtitle:
      'Powder は単一の Rust コアに 9 言語のバインディングを載せた ORM です。クエリ結果はカラムバッファ（PCB)として一度だけ言語境界を越えるため、行ごとの変換コストがありません。スキーマファイルひとつから型付きモデル・マイグレーション・検証が生成されます。',
    ctaPrimary: '5 分ではじめる',
    ctaSecondary: 'GitHub',
    installNote: 'インストールはパッケージマネージャーの 1 行だけ。',
    valuesHeading: 'なぜ Powder なのか',
    values: [
      {
        icon: <Gauge className="size-5" />,
        title: '本当に速い',
        body: '5 言語すべてのベンチマークで、その言語の素の SQL ドライバより高速。行ごとに FFI を往復せず、カラムバッファを一度だけ渡すからです。',
      },
      {
        icon: <Boxes className="size-5" />,
        title: 'ひとつのコア、ひとつのバグ',
        body: 'TS・Python・Kotlin の ORM とすべてのドライバが同じ Rust エンジンを共有。一箇所直せば、すべての言語が良くなります。',
      },
      {
        icon: <FileCode2 className="size-5" />,
        title: 'スキーマが唯一の真実',
        body: 'powder.schema.json ひとつから DDL・マイグレーション・ドリフト検証（ビルドゲート）・型付きモデル・リレーション・名前付きクエリがすべて生成されます。',
      },
      {
        icon: <ShieldCheck className="size-5" />,
        title: '正直に失敗する',
        body: 'マッピングできない型、全行を消しかねない delete、誤ったプレースホルダ——どれも黙って通過せず、解決策つきのエラーになります。',
      },
    ],
    demoHeading: '30 秒でわかる',
    demoSub: '読んだとおりに動く構文。N+1 のないリレーション読み込みも。',
    benchHeading: '数字で見る性能',
    bench: [
      { value: '3~4ms', label: 'コールドクエリ @ 20万行' },
      { value: '~25×', label: 'node:sqlite 比' },
      { value: '0.02ms', label: '繰り返しクエリ（キャッシュ）' },
    ],
    matrixHeading: 'サポート範囲',
    langsLabel: '言語',
    dbsLabel: 'データベース',
    sponsorHeading: 'Powder は役に立ちましたか？',
    sponsorBody:
      'Powder はオープンソースで開発されています。スポンサーはコア開発と、より多くの言語・DB サポートの大きな支えになります。',
    sponsorCta: '開発をスポンサーする',
    feedHeading: 'リアルタイム開発フィード',
    feedBody: 'main ブランチにコミットがあるたびに AI が要約してここに表示します。',
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
const DBS = ['SQLite', 'PostgreSQL', 'MySQL / MariaDB'];

export default async function HomePage({ params }: { params: Promise<{ lang: string }> }) {
  const { lang } = await params;
  const t = copy[lang] ?? copy.ko;

  return (
    <>
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

      {/* Live feed + Sponsor */}
      <section className="border-t border-fd-border">
        <div className="mx-auto max-w-5xl px-6 py-20">
          <div className="grid gap-10 md:grid-cols-2">
            <div>
              <h2 className="text-2xl font-semibold">{t.feedHeading}</h2>
              <p className="mt-3 text-fd-muted-foreground">{t.feedBody}</p>
              <div className="mt-6">
                <CommitFeed lang={lang} />
              </div>
            </div>
            <div className="flex flex-col items-center text-center md:items-start md:text-left">
              <div className="mb-4 inline-flex size-12 items-center justify-center rounded-full bg-fd-primary/10 text-fd-primary">
                <Heart className="size-6" />
              </div>
              <h2 className="text-2xl font-semibold">{t.sponsorHeading}</h2>
              <p className="mt-3 text-balance text-fd-muted-foreground">{t.sponsorBody}</p>
              <a
                href={SPONSOR_URL}
                target="_blank"
                rel="noreferrer"
                className="mt-6 inline-flex items-center gap-2 rounded-lg bg-fd-primary px-5 py-2.5 font-medium text-fd-primary-foreground transition-opacity hover:opacity-90"
              >
                <Heart className="size-4" />
                {t.sponsorCta}
              </a>
              <p className="mt-3 text-xs text-fd-muted-foreground">fairy.hada.io</p>
            </div>
          </div>

          <div className="mt-16 border-t border-fd-border pt-10">
            <VisitorStats lang={lang} />
          </div>
        </div>
      </section>

    </main>
    <SiteFooter lang={lang} />
    </>
  );
}
