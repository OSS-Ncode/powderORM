'use client';

import { useEffect, useState } from 'react';

type Copy = { today: string; total: string };

const T: Record<string, Copy> = {
  ko: { today: '오늘 방문자', total: '누적 방문자' },
  en: { today: 'Visitors today', total: 'Total visitors' },
  zh: { today: '今日访客', total: '累计访客' },
  ja: { today: '本日の訪問者', total: '累計訪問者' },
};

const VISITOR_ID_KEY = 'powder_visitor_id';

function getVisitorId(): string {
  let id = localStorage.getItem(VISITOR_ID_KEY);
  if (!id) {
    id = crypto.randomUUID();
    localStorage.setItem(VISITOR_ID_KEY, id);
  }
  return id;
}

export function VisitorStats({ lang }: { lang: string }) {
  const t = T[lang] ?? T.ko;
  const [stats, setStats] = useState<{ today: number; total: number } | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetch('/api/visit', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ visitor_id: getVisitorId() }),
    })
      .then((res) => res.json())
      .then((data) => {
        if (!cancelled && typeof data.today === 'number' && typeof data.total === 'number') {
          setStats({ today: data.today, total: data.total });
        }
      })
      .catch(() => {
        // 조용히 무시 — 방문자 수는 배경 정보라 실패해도 페이지엔 영향 없음
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="flex items-center justify-center gap-10 text-center">
      <div>
        <div className="text-2xl font-bold text-fd-primary">
          {stats ? stats.today.toLocaleString() : '—'}
        </div>
        <div className="mt-1 text-sm text-fd-muted-foreground">{t.today}</div>
      </div>
      <div>
        <div className="text-2xl font-bold text-fd-primary">
          {stats ? stats.total.toLocaleString() : '—'}
        </div>
        <div className="mt-1 text-sm text-fd-muted-foreground">{t.total}</div>
      </div>
    </div>
  );
}
