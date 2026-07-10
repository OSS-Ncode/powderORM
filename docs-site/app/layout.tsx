import { Inter } from 'next/font/google';
import './global.css';
import type { ReactNode } from 'react';

const inter = Inter({
  subsets: ['latin'],
});

// html/body live here (locale-neutral); the locale-aware Provider is in
// app/[lang]/layout.tsx. Static export can't run middleware, so the html lang
// attribute stays at the default locale — acceptable for a static docs site.
export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="ko" className={inter.className} suppressHydrationWarning>
      <body className="flex flex-col min-h-screen">{children}</body>
    </html>
  );
}
