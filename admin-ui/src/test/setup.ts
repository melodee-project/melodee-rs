import '@testing-library/jest-dom/vitest';

import { i18nReady } from '../i18n';

// Components render translated strings; the en-US source catalog must be
// registered before any test renders.
await i18nReady;

// jsdom lacks matchMedia; the theme provider and Carbon components use it.
if (typeof window.matchMedia !== 'function') {
  window.matchMedia = (query: string): MediaQueryList => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => undefined,
    removeListener: () => undefined,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    dispatchEvent: () => false,
  });
}

// jsdom lacks ResizeObserver; Carbon Modal/TextArea attach one.
if (typeof globalThis.ResizeObserver === 'undefined') {
  class ResizeObserverStub {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver;
}
