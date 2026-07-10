'use client';
import { useEffect } from 'react';

// Static export can't run middleware, so `/` redirects to the default locale
// client-side (with a <meta refresh> fallback for no-JS clients).
export default function RootRedirect() {
  useEffect(() => {
    window.location.replace('/ko');
  }, []);
  return (
    <>
      <meta httpEquiv="refresh" content="0; url=/ko" />
      <p style={{ padding: 24 }}>
        Redirecting to <a href="/ko">/ko</a>…
      </p>
    </>
  );
}
