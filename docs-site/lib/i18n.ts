import { defineI18n } from 'fumadocs-core/i18n';

export const i18n = defineI18n({
  defaultLanguage: 'ko',
  languages: ['ko', 'en'],
  // default language has no URL prefix in files; both locales are prefixed in routes
  hideLocale: 'never',
});
