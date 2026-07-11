import { DiscordMark } from './discord-mark';
import { SupportDialog } from './support-dialog';

const DISCORD_URL = 'https://discord.gg/SBg3Kgh4pZ';

type FooterCopy = {
  credit: string;
  discord: string;
};

const T: Record<string, FooterCopy> = {
  ko: { credit: 'Ncode Team이 만들었습니다', discord: '디스코드' },
  en: { credit: 'Built by the Ncode Team', discord: 'Discord' },
  zh: { credit: '由 Ncode Team 打造', discord: 'Discord' },
  ja: { credit: 'Ncode Team が開発', discord: 'Discord' },
};

// DocsLayout 사이드바의 GitHub/테마토글 줄 바로 아래에 들어가는 컴팩트 버전.
// 넓은 SiteFooter(홈페이지 전용)와 달리 좁은 사이드바 폭에 맞춘다.
export function SidebarFooter({ lang }: { lang: string }) {
  const t = T[lang] ?? T.ko;
  return (
    <div className="mt-2 flex flex-col gap-1.5 text-xs text-fd-muted-foreground">
      <span>{t.credit}</span>
      <div className="flex items-center gap-3">
        <a
          href={DISCORD_URL}
          target="_blank"
          rel="noreferrer"
          className="inline-flex items-center gap-1 transition-colors hover:text-fd-foreground"
        >
          <DiscordMark className="size-3.5" />
          {t.discord}
        </a>
        <SupportDialog lang={lang} />
      </div>
    </div>
  );
}
