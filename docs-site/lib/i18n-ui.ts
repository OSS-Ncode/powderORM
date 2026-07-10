import { defineI18nUI } from 'fumadocs-ui/i18n';
import { i18n } from './i18n';

// UI-string translations + language switcher metadata for RootProvider.
export const { provider } = defineI18nUI(i18n, {
  ko: {
    displayName: '한국어',
    search: '검색',
    searchNoResult: '검색 결과가 없어요',
    toc: '이 페이지에서',
    tocNoHeadings: '제목이 없어요',
    lastUpdate: '마지막 수정',
    chooseLanguage: '언어 선택',
    nextPage: '다음',
    previousPage: '이전',
    chooseTheme: '테마',
    editOnGithub: 'GitHub에서 편집',
  },
  en: {
    displayName: 'English',
  },
});
