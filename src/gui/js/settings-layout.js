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

let activeIndex = -1;

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
  if (index !== activeIndex) {
    activeIndex = index;
    revealInNav(items[index]);
  }
}

// The list can be longer than the sidebar: it glides along with the page, keeping the
// next section in view too.
function revealInNav(item) {
  const nav = item?.closest('.settings-nav');
  if (!nav || item.offsetParent === null) return;
  const box = nav.getBoundingClientRect();
  const row = item.getBoundingClientRect();
  const margin = row.height + 6;
  let by = 0;
  if (row.top < box.top + margin) by = row.top - box.top - margin;
  else if (row.bottom > box.bottom - margin) by = row.bottom - box.bottom + margin;
  if (!by) return;
  const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  nav.scrollBy({ top: by, behavior: reduced ? 'auto' : 'smooth' });
}

function currentSectionIndex() {
  // At the very bottom the last section is the one being read, even when it
  // is too short to reach the top.
  if (scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2) {
    const shown = sections.map((section, i) => (section.offsetParent !== null ? i : -1)).filter((i) => i >= 0);
    return shown.length ? shown[shown.length - 1] : 0;
  }
  const top = scroller.getBoundingClientRect().top + SPY_OFFSET_PX;
  let index = 0;
  sections.forEach((section, i) => {
    if (section.offsetParent !== null && section.getBoundingClientRect().top <= top) index = i;
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
  // A search may be hiding the section: going to one by name ends the search.
  const search = document.getElementById('settings-search');
  if (search && search.value) {
    search.value = '';
    search.dispatchEvent(new Event('input'));
  }
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

// Words people type that a setting's own text does not hold, by the id of its control.
const SEARCH_WORDS = {
  'fillground-toggle': 'underground stone ores mining survival',
  'caves-toggle': 'underground mining',
  'height-multiplier-slider': 'mountains elevation exaggeration flat',
  'canopy-height-toggle': 'forest trees',
  'winter-toggle': 'snow ice christmas',
  'voxy-lod-toggle': 'render distance far view',
  'dh-lod-toggle': 'render distance far view',
  'disable-height-limit-toggle': 'tall mountains y limit',
  'scale-value-slider': 'size smaller bigger',
  'language-select': 'sprache langue idioma translation',
  'clear-cache-button': 'disk space storage delete',
  'meld-rocks-toggle': 'stones boulders',
  'meld-bushes-toggle': 'shrubs plants',
  'meld-workers-select': 'workers processes performance speed faster ram memory large big',
  'meld-piece-size-group': 'pieces regions',
  'meld-ram-budget-input': 'ram limit',
  'meld-threads-input': 'cpu cores performance',
  'meld-seed-input': 'random variation',
  'meld-world-border-toggle': 'wall edge limit',
  'meld-min-y-input': 'height depth bottom',
  'meld-max-y-input': 'height top',
  'meld-road-detail-group': 'markings lines sidewalks footways',
  'meld-surface-patches-toggle': 'asphalt paving texture',
  'meld-buildings-toggle': 'houses terrain only',
  'meld-river-bed-group': 'water depth rivers canals',
  'meld-water-detail-group': 'rivers streams depth',
  'meld-climate-mode-group': 'biomes desert snow',
  'meld-offline-toggle': 'no internet network cache',
  'meld-overpass-input': 'server osm',
  'meld-osm-source-group': 'openstreetmap data geofabrik',
  'meld-origin-input': 'coordinates latitude longitude',
};

// What a search can match in a setting: its name, description, choices, tooltip and extra words.
function rowText(row) {
  const tips = Array.from(row.querySelectorAll('[data-tooltip]'), (t) => t.dataset.tooltip).join(' ');
  const extra = Array.from(row.querySelectorAll('[id]'), (el) => SEARCH_WORDS[el.id] || '').join(' ');
  // A greyed row's reason is about another setting, so it is no reason to list this one.
  const reason = row.querySelector('.setting-reason')?.textContent || '';
  return `${row.textContent.replace(reason, '')} ${tips} ${extra}`.toLowerCase();
}

// The search field in the sidebar leaves only the settings that hold every word typed, in the
// chosen language or in English (the page's text before it is translated).
function initSearch() {
  const input = document.getElementById('settings-search');
  const content = document.querySelector('#settings-modal .settings-content');
  if (!input || !content) return;
  const rows = Array.from(content.querySelectorAll('.settings-row'));
  const english = new Map(rows.map((row) => [row, rowText(row)]));
  const empty = document.getElementById('settings-search-empty');
  const apply = () => {
    const words = input.value.toLowerCase().split(/\s+/).filter(Boolean);
    const searching = words.length > 0;
    const advancedOff = document.body.classList.contains('meld-off');
    rows.forEach((row) => {
      const hay = searching ? `${rowText(row)} ${english.get(row)}` : '';
      row.classList.toggle('search-miss', searching && !words.every((w) => hay.includes(w)));
    });
    let any = false;
    content.querySelectorAll('.settings-section').forEach((section) => {
      const hit = !!section.querySelector('.settings-row:not(.search-miss)');
      section.classList.toggle('search-miss', searching && !hit);
      if (hit && !(advancedOff && section.classList.contains('meld-only'))) any = true;
    });
    content.querySelectorAll('.settings-group-title').forEach((title) => {
      const grid = title.nextElementSibling;
      title.classList.toggle('search-miss', searching && !(grid && grid.querySelector('.settings-row:not(.search-miss)')));
    });
    if (empty) empty.hidden = !searching || any;
    if (scroller) {
      if (searching) scroller.scrollTop = 0;
      pinned = null;
      setActive(currentSectionIndex());
    }
  };
  input.addEventListener('input', apply);
  // Reset all settings or the Advanced switch can change what there is to find.
  document.addEventListener(SETTINGS_REFRESHED_EVENT, () => input.value && apply());
  // Escape empties the field first; pressed again it closes Settings as usual.
  input.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape' || !input.value) return;
    input.value = '';
    apply();
    e.stopPropagation();
  });
}

export function initSettingsLayout() {
  initNav();
  initSearch();
  document
    .querySelectorAll('#settings-modal .choice-cards[data-select]')
    .forEach(initSelectCards);
}

// Called when the page opens: the scroll position survives a close, so the
// highlight has to be read from it again rather than trusted.
export function syncSettingsLayout() {
  // A search left from last time would hide most of the page.
  const search = document.getElementById('settings-search');
  if (search && search.value) {
    search.value = '';
    search.dispatchEvent(new Event('input'));
  }
  if (!scroller) return;
  pinned = null;
  setActive(currentSectionIndex());
  refreshModifiedDots();
}
