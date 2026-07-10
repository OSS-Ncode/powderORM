import { Provider } from '@/components/provider';
import { i18n } from '@/lib/i18n';
import type { ReactNode } from 'react';

export default async function LangLayout({
  params,
  children,
}: {
  params: Promise<{ lang: string }>;
  children: ReactNode;
}) {
  const { lang } = await params;
  return <Provider locale={lang}>{children}</Provider>;
}

export function generateStaticParams() {
  return i18n.languages.map((lang) => ({ lang }));
}
