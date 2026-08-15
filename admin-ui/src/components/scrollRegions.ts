import { useEffect } from 'react';

/**
 * Keyboard access to scrollable regions (WCAG 2.1.1; axe
 * scrollable-region-focusable, UI-SPEC §9).
 *
 * Carbon's `.cds--data-table-content` and the Melodee overflow wrappers
 * scroll horizontally below the lg breakpoint, but neither is
 * keyboard-focusable — keyboard-only users cannot scroll them at all. This
 * hook gives any region that actually overflows a `tabindex="0"` (removing
 * it again when the region stops overflowing, so desktop tab order gains no
 * extra stops).
 *
 * Two non-obvious constraints shape the implementation:
 *  - The Carbon div is not reachable from our JSX, so the marking happens
 *    from the shell via DOM observation rather than props.
 *  - React may replace the content root node after effects run (the Carbon
 *    HeaderContainer render-prop subtree re-mounts), so the observer must
 *    watch `document.body` and re-resolve the root on every pass instead of
 *    capturing a node once.
 */
const SCROLL_REGION_SELECTOR = [
  '.cds--data-table-content',
  '.melodee-review-table-scroll',
  '.melodee-table-scroll',
  '.melodee-onboarding-checks-scroll',
  '.melodee-onboarding-steps-scroll',
  '.melodee-queue-chips',
].join(',');

function updateScrollRegions(root: HTMLElement): void {
  root.querySelectorAll(SCROLL_REGION_SELECTOR).forEach((el) => {
    if (!(el instanceof HTMLElement)) {
      return;
    }
    const overflows =
      el.scrollWidth - el.clientWidth > 1 || el.scrollHeight - el.clientHeight > 1;
    if (overflows && el.tabIndex < 0) {
      el.tabIndex = 0;
    } else if (!overflows && el.tabIndex === 0) {
      // tabIndex defaults to -1 on these containers; 0 means we set it.
      el.removeAttribute('tabindex');
    }
  });
}

/** Keeps overflowing scroll regions inside the main content focusable. */
export function useScrollRegionFocus(rootId = 'main-content'): void {
  useEffect(() => {
    let cancelled = false;
    const update = () => {
      if (cancelled) {
        return;
      }
      const root = document.getElementById(rootId);
      if (root) {
        updateScrollRegions(root);
      }
    };
    update();
    // Watch body, not the root: the root node can be replaced wholesale, and
    // tabindex itself is excluded so our own writes never retrigger this.
    const observer = new MutationObserver(update);
    observer.observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ['class', 'style'],
    });
    window.addEventListener('resize', update);
    // Webfont arrival reflows text metrics without any DOM mutation, and can
    // turn a non-overflowing region into a scrollable one.
    if (document.fonts?.ready) {
      void document.fonts.ready.then(update);
    }
    return () => {
      cancelled = true;
      observer.disconnect();
      window.removeEventListener('resize', update);
    };
  }, [rootId]);
}
