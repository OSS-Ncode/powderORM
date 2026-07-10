import { source } from '@/lib/source';
import { createFromSource } from 'fumadocs-core/search/server';

export const revalidate = false;

// i18n search: map each locale to an Orama-compatible language. Orama has no
// Korean stemmer, so `ko` uses the default tokenizer (whitespace) instead of
// crashing with LANGUAGE_NOT_SUPPORTED.
export const { staticGET: GET } = createFromSource(source, {
  localeMap: {
    en: { language: 'english' },
    ko: {},
  },
});
