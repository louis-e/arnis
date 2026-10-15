// Layout behaviour of the Settings page: the section sidebar and the cards
// that stand in for a <select>. Values, persistence and revert stay with
// main.js and settings-store.js; nothing here owns a setting.

// Emitted by settings-store.js whenever the modified markers are recomputed.
export const SETTINGS_REFRESHED_EVENT = 'arnis:settings-refreshed';

// How far below the top of the scroll area a section has to reach before the
// sidebar calls it the current one.
const SPY_OFFSET_PX = 96;

// Space left above a section the sidebar scrolls to.
const JUMP_GAP_PX = 16;

let scroller = null;
let items = [];
let sections = [];

// Set while the view shows the section the user clicked in the sidebar. A
// short last section can never reach the top, so without this the highlight
// would jump to whichever section the scroll happened to end on. Cleared by
// the next scroll the user makes themselves.
let pinned = null;

function setActive(index) {
  items.forEach((item, i) => {
    const on = i === index;
    item.classList.toggle('active', on);
    if (on) {
      item.setAttribute('aria-current', 'true');
    } else {
      item.removeAttribute('aria-current');
    }
  });
}

function currentSectionIndex() {
  // At the very bottom the last section is the one being read, even when it
  // is too short to reach the top.
  if (scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2) {
    return sections.length - 1;
  }
  const top = scroller.getBoundingClientRect().top + SPY_OFFSET_PX;
  let index = 0;
  sections.forEach((section, i) => {
    if (section.getBoundingClientRect().top <= top) index = i;
  });
  return index;
}

function onScroll() {
  if (pinned !== null) return;
  setActive(currentSectionIndex());
}

function unpin() {
  pinned = null;
}

function jumpTo(index) {
  const section = sections[index];
  if (!section) return;
  pinned = index;
  setActive(index);
  const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  scroller.scrollTo({
    top: Math.max(0, section.offsetTop - JUMP_GAP_PX),
    behavior: reduced ? 'auto' : 'smooth',
  });
}

// A dot on every section that holds a setting changed from its default, so a
// change made weeks ago can be found without scrolling through everything.
function refreshModifiedDots() {
  items.forEach((item, i) => {
    const section = sections[i];
    const modified = !!(section && section.querySelector('.settings-row.is-modified'));
    item.classList.toggle('has-modified', modified);
  });
}

function initNav() {
  scroller = document.querySelector('#settings-modal .settings-scrollable');
  if (!scroller) return;

  items = Array.from(document.querySelectorAll('#settings-modal .settings-nav-item'));
  sections = items.map((item) => document.getElementById(item.dataset.target));
  if (sections.some((section) => !section)) {
    console.warn('Settings sidebar points at a section that does not exist.');
    return;
  }

  items.forEach((item, i) => {
    item.addEventListener('click', () => jumpTo(i));
  });

  scroller.addEventListener('scroll', onScroll, { passive: true });
  // Only input the user makes releases the pin; the smooth scroll started by
  // jumpTo() fires scroll events of its own and must not.
  scroller.addEventListener('wheel', unpin, { passive: true });
  scroller.addEventListener('touchstart', unpin, { passive: true });
  scroller.addEventListener('pointerdown', unpin);
  scroller.addEventListener('keydown', unpin);

  document.addEventListener(SETTINGS_REFRESHED_EVENT, refreshModifiedDots);

  setActive(0);
  refreshModifiedDots();
}

// Cards that present a <select>. The select stays the source of truth: the
// generation request, the settings store and the Earth-only gate all read or
// write it, and each of those writes ends in a change event or a disabled
// flip, which is what the cards follow.
function initSelectCards(group) {
  const select = document.getElementById(group.dataset.select);
  if (!select) return;
  const cards = Array.from(group.querySelectorAll('.segment[data-value]'));

  const sync = () => {
    cards.forEach((card) => {
      card.classList.toggle('active', card.dataset.value === select.value);
      card.disabled = select.disabled;
    });
  };

  cards.forEach((card) => {
    card.addEventListener('click', () => {
      if (select.value === card.dataset.value) return;
      select.value = card.dataset.value;
      select.dispatchEvent(new Event('input', { bubbles: true }));
      select.dispatchEvent(new Event('change', { bubbles: true }));
    });
  });

  select.addEventListener('change', sync);
  // setCelestialBody() sets `disabled` from code, which fires no event.
  new MutationObserver(sync).observe(select, {
    attributes: true,
    attributeFilter: ['disabled'],
  });
  sync();
}

export function initSettingsLayout() {
  initNav();
  document
    .querySelectorAll('#settings-modal .choice-cards[data-select]')
    .forEach(initSelectCards);
}

// Called when the page opens: the scroll position survives a close, so the
// highlight has to be read from it again rather than trusted.
export function syncSettingsLayout() {
  if (!scroller) return;
  pinned = null;
  setActive(currentSectionIndex());
  refreshModifiedDots();
}
