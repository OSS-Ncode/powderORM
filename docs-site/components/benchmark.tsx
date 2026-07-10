'use client';
import { useState } from 'react';

// Headline benchmark: sort (ORDER BY), 50,000 rows × 5 trials, median ms.
// Numbers from docs/reports/2026-07-09-benchmarks-multilang.md — the same query,
// data, and checksum contract across engines. Compare *within a tab*, not across
// languages (raw-driver speed is a property of that language's driver).
type Engine = { name: string; ms: number; kind: 'powder' | 'raw' | 'orm' };
type Lang = { key: string; label: string; engines: Engine[] };

const DATA: Lang[] = [
  {
    key: 'js',
    label: 'TypeScript / JS',
    engines: [
      { name: 'Powder', ms: 2.91, kind: 'powder' },
      { name: 'better-sqlite3', ms: 11.2, kind: 'raw' },
      { name: 'Prisma', ms: 24.57, kind: 'orm' },
    ],
  },
  {
    key: 'python',
    label: 'Python',
    engines: [
      { name: 'Powder', ms: 4.23, kind: 'powder' },
      { name: 'sqlite3', ms: 23.07, kind: 'raw' },
      { name: 'Peewee', ms: 67.9, kind: 'orm' },
    ],
  },
  {
    key: 'rust',
    label: 'Rust',
    engines: [
      { name: 'Powder', ms: 2.84, kind: 'powder' },
      { name: 'rusqlite', ms: 3.94, kind: 'raw' },
      { name: 'Diesel', ms: 8.75, kind: 'orm' },
    ],
  },
  {
    key: 'java',
    label: 'Java',
    engines: [
      { name: 'Powder', ms: 3.98, kind: 'powder' },
      { name: 'JDBC', ms: 17.2, kind: 'raw' },
      { name: 'ORMLite', ms: 25.07, kind: 'orm' },
    ],
  },
  {
    key: 'go',
    label: 'Go',
    engines: [
      { name: 'Powder', ms: 4.08, kind: 'powder' },
      { name: 'database/sql', ms: 29.46, kind: 'raw' },
      { name: 'GORM', ms: 50.56, kind: 'orm' },
    ],
  },
];

const T = {
  ko: {
    caption: '정렬 (ORDER BY) · 50,000행 × 5회 · 중앙값 ms — 낮을수록 빠릅니다',
    kind: { powder: 'Powder', raw: 'raw 드라이버', orm: '대표 ORM' } as Record<string, string>,
    faster: (n: string) => `Powder보다 ${n}× 느림`,
    baseline: '기준 (가장 빠름)',
    note: '언어마다 raw 드라이버 특성이 달라 절대값의 언어 간 비교보다 탭 안 비교가 의미 있어요. 필터·집계·조인 시나리오는 저장소에서 라이브로 돌려볼 수 있습니다: cd bench-site && npm start',
  },
  en: {
    caption: 'Sort (ORDER BY) · 50,000 rows × 5 · median ms — lower is faster',
    kind: { powder: 'Powder', raw: 'raw driver', orm: 'typical ORM' } as Record<string, string>,
    faster: (n: string) => `${n}× slower than Powder`,
    baseline: 'baseline (fastest)',
    note: 'Raw-driver speed differs by language, so compare within a tab rather than across languages by absolute value. Run the filter / aggregate / join scenarios live from the repo: cd bench-site && npm start',
  },
};

export function BenchmarkExplorer({ locale = 'ko' }: { locale?: 'ko' | 'en' }) {
  const [active, setActive] = useState(DATA[0].key);
  const t = T[locale] ?? T.ko;
  const lang = DATA.find((l) => l.key === active) ?? DATA[0];
  const max = Math.max(...lang.engines.map((e) => e.ms));
  const powderMs = lang.engines.find((e) => e.kind === 'powder')?.ms ?? 1;

  return (
    <div className="not-prose my-6 rounded-xl border border-fd-border bg-fd-card p-4 sm:p-6">
      {/* language tabs */}
      <div role="tablist" aria-label="benchmark language" className="mb-1 flex flex-wrap gap-1.5">
        {DATA.map((l) => {
          const on = l.key === active;
          return (
            <button
              key={l.key}
              role="tab"
              aria-selected={on}
              onClick={() => setActive(l.key)}
              className={
                'rounded-md px-3 py-1.5 text-sm font-medium transition-colors ' +
                (on
                  ? 'bg-fd-primary text-fd-primary-foreground'
                  : 'border border-fd-border bg-fd-background text-fd-muted-foreground hover:bg-fd-accent')
              }
            >
              {l.label}
            </button>
          );
        })}
      </div>

      <p className="mb-5 mt-3 text-xs text-fd-muted-foreground">{t.caption}</p>

      {/* bars */}
      <div className="flex flex-col gap-3">
        {lang.engines.map((e) => {
          const pct = Math.max(2, (e.ms / max) * 100);
          const mult = (e.ms / powderMs).toFixed(1);
          const isPowder = e.kind === 'powder';
          return (
            <div key={e.name}>
              <div className="mb-1 flex items-baseline justify-between gap-3 text-sm">
                <span className={isPowder ? 'font-semibold text-fd-foreground' : 'text-fd-foreground'}>
                  {e.name}
                  <span className="ml-2 text-xs font-normal text-fd-muted-foreground">
                    {t.kind[e.kind]}
                  </span>
                </span>
                <span className="tabular-nums text-fd-muted-foreground">
                  {e.ms.toFixed(2)} ms
                  <span className="ml-2 text-xs">
                    {isPowder ? t.baseline : t.faster(mult)}
                  </span>
                </span>
              </div>
              <div className="h-3 w-full overflow-hidden rounded-full bg-fd-muted">
                <div
                  className="h-full rounded-full transition-[width] duration-500 ease-out"
                  style={{
                    width: `${pct}%`,
                    background: isPowder
                      ? 'var(--color-fd-primary)'
                      : 'color-mix(in oklab, var(--color-fd-muted-foreground) 45%, transparent)',
                  }}
                  title={`${e.name}: ${e.ms.toFixed(2)} ms`}
                />
              </div>
            </div>
          );
        })}
      </div>

      <p className="mt-5 text-xs leading-relaxed text-fd-muted-foreground">{t.note}</p>
    </div>
  );
}
