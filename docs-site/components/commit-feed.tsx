'use client';

import { useEffect, useRef, useState } from 'react';

type FeedEntry = {
  sha: string;
  short_sha: string;
  author: string;
  message: string;
  summary: Partial<Record<'ko' | 'en' | 'zh' | 'ja', string | null>>;
  timestamp: string;
  url: string;
};

type FeedCopy = {
  empty: string;
};

const T: Record<string, FeedCopy> = {
  ko: { empty: '아직 표시할 커밋이 없습니다.' },
  en: { empty: 'No commits yet.' },
  zh: { empty: '暂时没有提交记录。' },
  ja: { empty: 'まだコミットがありません。' },
};

const POLL_INTERVAL_MS = 10_000;
const NEAR_BOTTOM_PX = 40;

function relativeTime(iso: string, lang: string): string {
  const diffMs = Date.now() - new Date(iso).getTime();
  const minutes = Math.max(0, Math.floor(diffMs / 60_000));
  if (minutes < 1) {
    return { ko: '방금', en: 'just now', zh: '刚刚', ja: 'たった今' }[lang] ?? 'just now';
  }
  if (minutes < 60) {
    return { ko: `${minutes}분 전`, en: `${minutes}m ago`, zh: `${minutes} 分钟前`, ja: `${minutes}分前` }[
      lang
    ] ?? `${minutes}m ago`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return { ko: `${hours}시간 전`, en: `${hours}h ago`, zh: `${hours} 小时前`, ja: `${hours}時間前` }[
      lang
    ] ?? `${hours}h ago`;
  }
  const days = Math.floor(hours / 24);
  return { ko: `${days}일 전`, en: `${days}d ago`, zh: `${days} 天前`, ja: `${days}日前` }[lang] ?? `${days}d ago`;
}

export function CommitFeed({ lang }: { lang: string }) {
  const t = T[lang] ?? T.ko;
  const [entries, setEntries] = useState<FeedEntry[]>([]);
  const containerRef = useRef<HTMLDivElement>(null);
  const shasRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    let cancelled = false;

    async function poll() {
      try {
        const res = await fetch('/commit-feed.json', { cache: 'no-store' });
        if (!res.ok) return;
        const data: FeedEntry[] = await res.json();
        if (cancelled) return;

        const container = containerRef.current;
        const wasNearBottom = container
          ? container.scrollHeight - container.scrollTop - container.clientHeight < NEAR_BOTTOM_PX
          : true;

        const knownShas = shasRef.current;
        const hasNew = data.some((e) => !knownShas.has(e.sha));
        shasRef.current = new Set(data.map((e) => e.sha));

        setEntries(data);

        if (hasNew && wasNearBottom) {
          requestAnimationFrame(() => {
            container?.scrollTo({ top: container.scrollHeight });
          });
        }
      } catch {
        // 조용히 무시하고 다음 폴링에서 재시도
      }
    }

    poll();
    const id = setInterval(poll, POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, []);

  return (
    <div
      ref={containerRef}
      className="h-96 w-full overflow-y-auto rounded-xl border border-fd-border bg-fd-card"
    >
      {entries.length === 0 ? (
        <p className="flex h-full items-center justify-center text-sm text-fd-muted-foreground">
          {t.empty}
        </p>
      ) : (
        <div className="flex flex-col gap-2 p-4">
          {entries.map((e) => (
            <a
              key={e.sha}
              href={e.url}
              target="_blank"
              rel="noreferrer"
              className="rounded-lg border border-fd-border bg-fd-background p-3 text-sm transition-colors hover:border-fd-primary/40"
            >
              <p className="text-fd-foreground">
                {e.summary[lang as 'ko' | 'en' | 'zh' | 'ja'] ?? e.summary.ko ?? e.message}
              </p>
              <p className="mt-1.5 text-xs text-fd-muted-foreground">
                {e.author} · {e.short_sha} · {relativeTime(e.timestamp, lang)}
              </p>
            </a>
          ))}
        </div>
      )}
    </div>
  );
}
