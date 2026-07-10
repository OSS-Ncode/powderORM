'use client';
import SearchDialog from '@/components/search';
import { RootProvider } from 'fumadocs-ui/provider/next';
import { type ReactNode } from 'react';
import { provider } from '@/lib/i18n-ui';

export function Provider({ locale, children }: { locale: string; children: ReactNode }) {
  return (
    <RootProvider search={{ SearchDialog }} i18n={provider(locale)}>
      {children}
    </RootProvider>
  );
}
