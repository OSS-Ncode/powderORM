import Link from 'next/link';
import { DiscordMark } from './discord-mark';

const DISCORD_URL = 'https://discord.gg/SBg3Kgh4pZ';

type FooterCopy = {
  credit: string;
  discord: string;
  contact: string;
};

const T: Record<string, FooterCopy> = {
  ko: {
    credit: 'Ncode Team이 만들었습니다',
    discord: '디스코드 커뮤니티',
    contact: '문의하기',
  },
  en: {
    credit: 'Built by the Ncode Team',
    discord: 'Discord community',
    contact: 'Contact us',
  },
  zh: {
    credit: '由 Ncode Team 打造',
    discord: 'Discord 社区',
    contact: '联系我们',
  },
  ja: {
    credit: 'Ncode Team が開発しています',
    discord: 'Discord コミュニティ',
    contact: 'お問い合わせ',
  },
};

export function SiteFooter({ lang }: { lang: string }) {
  const t = T[lang] ?? T.ko;
  return (
    <footer className="border-t border-fd-border">
      <div className="mx-auto flex w-full max-w-5xl flex-col items-center gap-3 px-6 py-8 text-sm text-fd-muted-foreground sm:flex-row sm:justify-between">
        <span>{t.credit}</span>
        <div className="flex items-center gap-5">
          <Link href={`/${lang}#support`} className="transition-colors hover:text-fd-foreground">
            {t.contact}
          </Link>
          <a
            href={DISCORD_URL}
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center gap-1.5 transition-colors hover:text-fd-foreground"
          >
            <DiscordMark className="size-4" />
            {t.discord}
          </a>
        </div>
      </div>
    </footer>
  );
}
