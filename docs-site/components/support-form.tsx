'use client';

import { useState, type FormEvent } from 'react';

export type SupportFormCopy = {
  emailLabel: string;
  emailPlaceholder: string;
  messageLabel: string;
  messagePlaceholder: string;
  submit: string;
  submitting: string;
  success: string;
  error: string;
};

const WEB3FORMS_ENDPOINT = 'https://api.web3forms.com/submit';

type Status = 'idle' | 'submitting' | 'success' | 'error';

export function SupportForm({ copy }: { copy: SupportFormCopy }) {
  const [email, setEmail] = useState('');
  const [message, setMessage] = useState('');
  const [status, setStatus] = useState<Status>('idle');

  async function handleSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    setStatus('submitting');
    try {
      const res = await fetch(WEB3FORMS_ENDPOINT, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          access_key: process.env.NEXT_PUBLIC_WEB3FORMS_ACCESS_KEY,
          subject: 'Powder 문의',
          email,
          message,
        }),
      });
      const data = await res.json();
      if (data.success) {
        setStatus('success');
        setEmail('');
        setMessage('');
      } else {
        setStatus('error');
      }
    } catch {
      setStatus('error');
    }
  }

  return (
    <form onSubmit={handleSubmit} className="mt-6 flex w-full max-w-md flex-col gap-3">
      <input
        type="checkbox"
        name="botcheck"
        className="hidden"
        style={{ display: 'none' }}
        tabIndex={-1}
        autoComplete="off"
      />
      <input
        type="email"
        required
        value={email}
        onChange={(e) => setEmail(e.target.value)}
        placeholder={copy.emailPlaceholder}
        aria-label={copy.emailLabel}
        className="rounded-lg border border-fd-border bg-fd-card px-4 py-2.5 text-sm outline-none focus:border-fd-primary"
      />
      <textarea
        required
        rows={4}
        value={message}
        onChange={(e) => setMessage(e.target.value)}
        placeholder={copy.messagePlaceholder}
        aria-label={copy.messageLabel}
        className="resize-none rounded-lg border border-fd-border bg-fd-card px-4 py-2.5 text-sm outline-none focus:border-fd-primary"
      />
      <button
        type="submit"
        disabled={status === 'submitting'}
        className="inline-flex items-center justify-center gap-2 rounded-lg bg-fd-primary px-5 py-2.5 font-medium text-fd-primary-foreground transition-opacity hover:opacity-90 disabled:opacity-60"
      >
        {status === 'submitting' ? copy.submitting : copy.submit}
      </button>
      {status === 'success' && (
        <p className="text-sm text-emerald-600 dark:text-emerald-400">{copy.success}</p>
      )}
      {status === 'error' && (
        <p className="text-sm text-red-600 dark:text-red-400">{copy.error}</p>
      )}
    </form>
  );
}
