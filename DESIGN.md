# Window design

How the Arnis desktop window looks and why. Read this before you add a control, a colour, a map line or a string. The window is plain HTML, CSS and ES modules in `src/gui/`; there is no build step and no npm package.

## 1. What the window is for

One job: pick an area on a map and turn it into a Minecraft world. Everything on the first screen serves that: the map, the selection, Start Generation, the progress bar. Everything else is a setting, and settings live on the settings page, not on the main screen.

Rules that follow from it:

- The map comes first. Nothing sits on it that does not help to choose an area or to watch a build.
- A person who never opens Settings still gets a good world. Defaults should give a good world without opening Settings.
- Advanced features (big worlds in cells, offline data, finer control) are off by default. Off, the window matches stock Arnis (section 7).
- The window is at least 1000 x 650 and always dark (`tauri.conf.json`: `minWidth`, `minHeight`, `theme`).

## 2. Colour tokens

Neutral greys plus one yellow. Yellow means "selected", "on" or "primary" and nothing else. Green and red only show an outcome (done, failed). Do not add a hue.

The settings page and the dialogs use the `--ui-*` tokens in `src/gui/css/styles.css` (`:root`, lines 15-37):

| Token | Value | Use |
|---|---|---|
| `--primary-accent` | `#fecc44` | the yellow; `--ui-accent` points at it |
| `--primary-accent-dark`, `--primary-accent-light` | `#e6b73d`, `#fff2cc` | hover and tint of the yellow |
| `--ui-bg` | `#333333` | page background |
| `--ui-sidebar` | `#2a2a2a` | settings sidebar |
| `--ui-card`, `--ui-card-border` | `#3a3a3a`, `#444444` | section cards |
| `--ui-tile`, `--ui-tile-hover`, `--ui-tile-border` | `#424242`, `#484848`, `#4d4d4d` | setting tiles |
| `--ui-field`, `--ui-field-border` | `#2c2c2c`, `#5a5a5a` | inputs and selects |
| `--ui-text`, `--ui-muted`, `--ui-faint` | `#f2f2f2`, `#b8b8b8`, `#8f8f8f` | text, descriptions, hints |
| `--ui-accent-soft` | `rgba(254,204,68,.12)` | selected-row wash |
| `--ui-link` | `#ffd970` | links |
| `--ui-on-accent` | `#1c1c1c` | text on a yellow fill |
| `--ui-danger` | `#cf6679` | errors |

Things that float over the map use their own set, in `src/gui/css/map-controls.css` (`:root`, lines 12-18): `--map-surface` (translucent `#2e2e2e`), `--map-surface-solid`, `--map-border`, `--map-text`, `--map-muted`, `--map-accent` (the same yellow) and `--map-font`. The map frame is a separate page (`src/gui/maps.html`), so tokens from `styles.css` do not reach it; `map-controls.css` and `bbox.css` do.

Use a token. A new literal colour needs a reason in the commit message.

The colours of the cells on the map are not tokens. They live in one table, `arnisPieceStyle` in `src/gui/js/piece-style.js`, because the map and the legend must read the same values (section 6).

## 3. Type and spacing

- Font: `Inter, Avenir, Helvetica, Arial, sans-serif`, 16px over 24px (`styles.css`, first `:root` rule). Nothing is bundled, so Windows shows the system fallback. The map controls use the system stack `--map-font`. Code and paths in dialogs use `ui-monospace, "Cascadia Code", Menlo, Consolas, monospace`.
- Settings page sizes (`src/gui/css/settings.css`):

| Element | Size and weight |
|---|---|
| Page title (`.settings-topbar h2`) | 22px, 700 |
| Section title (`.settings-section-head h3`) | 17px, 700 |
| Setting name (`.setting-heading`), choice title, nav title | 14px, 600 |
| Segments, fields, buttons | 13px |
| Slider readout (`.setting-value`) | 13.5px, 700, tabular numbers |
| Setting description (`.setting-desc`), tooltip | 12px, muted |
| Nav subtitle | 11.5px, faint, two lines at most |

- Radius: 12px section card; 10px tiles, choice cards, dialogs; 8px nav items, fields, buttons; 6px segments and tooltip boxes; 50% round icon buttons.
- Spacing: 10px gap between tiles (`.setting-grid`), 18px section padding (`.settings-section`), content column at most 980px wide. The sidebar is 250px, 232px under 1100px. The grid keeps two tile columns at 1000px.
- Shadows are rare: dialogs and floating cards on the map only. The settings page is flat surfaces with 1px borders.
- Motion is short (0.15s to 0.2s) and every animation stops under `prefers-reduced-motion` (`settings.css`, `styles.css`, `bbox.css`). A running cell's dash animation is no exception.
- Focus: `:focus-visible` gets a 2px yellow outline. Keep it.
- Icons: one stroke sprite at the top of `src/gui/index.html` (`<symbol id="i-*">`, 24px viewBox, Lucide paths). Use `<svg class="icon"><use href="#i-NAME"></use></svg>`; stroke and size come from `.icon` in `styles.css`. To add one, copy the 24px path into the sprite. No icon font, no filled icons, no second source.
- Copy: Title Case for section and setting names (`Terrain &amp; Nature`; the legend heading is a plain caption), one plain sentence with a full stop for a description, a single verb on a button. Say why a control is unavailable instead of only greying it. Never write "Meld" in text a person reads; the switch is called Advanced features. (The exception is two credit strings: `advanced_meld_features_desc` and `about_meld_credit`.)

## 4. Page layout

`src/gui/index.html` holds everything: the sprite, the main page, the settings page (`#settings-modal`) and the dialogs. The map is a second page, `src/gui/maps.html`, in a frame; `src/gui/js/bbox.js` runs inside it and talks to the window with `postMessage` (`postToMap` in `src/gui/js/meld-features.js`).

Two layouts share the same markup. `body.map-first` (set by `initMeldFeatures` in `meld-features.js`) turns the second one on; CSS does the rest (`styles.css`, the "Map-first main page" block). The choice is the setting `layout-select` (Auto, Map, Classic), also reachable as `layout-about-select` under About. Map is the default for everyone.

Map-first:

- The map fills the window.
- Logo top centre, no box behind it; a click opens the site, hover shows the version.
- Map tools (draw, zoom) top left. City search bottom left, its results open upward. Credits (`.footer`) bottom centre.
- One stack of cards on the right (`.controls-box`): Start Generation and the world line, Java or Bedrock, the settings button, the plan, the progress bar. At 1280px nothing in it wraps.
- The 3D preview is a card bottom right (`.preview3d-mini`), resizable by its corner, 200px to 60% of the window. A click opens the full 3D view at the level of the map.
- Pressing Start on a big advanced build first shows a confirm card (`#run-confirm`) with the plan and a hold-to-start button. Small plain builds start on one click.

Classic is the stock Arnis 3.4.0 page: map left, panel right, footer under. Every rule for the new page starts with `body.map-first`, so Classic is untouched.

Rules for both:

- Nothing may cover the map tools or Leaflet's attribution.
- A plan and a selection never draw on top of each other unreadably: the selection is one outline, cells are thin outlines with a faint fill, the selection fades while a build runs.
- On a selection change, clear the old plan before drawing the new one, and drop a plan reply that belongs to an older selection (`planSeq` in `meld-features.js`).
- Real screenshots are in `docs/images/3.5.0/` (for example `gui-main-default.png`, `gui-main-classic.png`, `gui-main-plan.png`, `gui-card-plan-idle.png`). Do not add mock-ups.

## 5. Settings pages

The settings page is a full-window view: a section sidebar (`.settings-nav`) and one scrolling column of section cards (`.settings-section`). The sections, in order, have the ids `settings-section-world`, `-projects`, `-generation`, `-nature`, `-facades`, `-gameplay`, `-map`, `-application`, `-advanced`, `-about`. `src/gui/js/settings-layout.js` owns the sidebar, scroll spy and the cards that stand in for a `<select>`. `src/gui/js/settings-store.js` owns saving, the per-row revert arrow, the "modified" dot and Reset all.

Rules:

1. Every control sits in a `.settings-row` inside a `.setting-grid` of its section, with a stable `id`.
2. Every control has one entry in the `SETTINGS` array in `settings-store.js`. That entry is what gives it saving, revert and reset. A control without one silently loses its value on restart.
3. The default is written in the HTML (`checked`, `value`, `selected`, or `data-default` on a segment). Never set a default from JS: revert and Reset read the HTML.
4. Descriptions (`.setting-desc`) say what the control does in one or two short sentences. The `?` tooltip (`.tooltip-icon`, with `data-tooltip` and `data-tooltip-key`) holds the long version: cost, compatibility, version.
5. Hide a setting when it means nothing in the current state; grey it when it applies but not now, and say why. A master switch hides its group and disables its controls.
6. Every rule in `settings.css` starts with `.settings-page`. The generic dark-mode `button:hover` rules in `styles.css` would win over a single class.

### Add a setting

1. Add a `.settings-row.setting-tile` to the right section in `index.html`. Copy a neighbour of the same kind (switch, `select.settings-select`, `.segmented` group, slider, text field). Use `is-wide` for a row with a long control. Give the label an icon from the sprite, the name `data-localize`, the description `data-localize`, the tooltip `data-tooltip-key`.
2. Add `{ id, kind, store: OWN }` to `SETTINGS`. `kind` is `checkbox`, `select`, `number`, `text` or `segmented` (a segmented group also needs `valueAttr`, the attribute on the active segment).
3. Add the strings to all 20 locale files (section 8).
4. If the value reaches the build: read it where the other values are read in `main.js` and pass it in the `invoke("gui_start_generation", { ... })` call, add a snake_case parameter to `gui_start_generation` in `src/gui.rs`, and set the field in the one `Args { .. }` literal there. Settings that only send a CLI flag go in `meldFlags` (`meld-features.js`) instead.
5. If it is an advanced setting, put `meld-only` on the row (section 7).
6. If a picture explains it, add a setting preview (section 9).

### Add a tab (section)

1. Add a `<button class="settings-nav-item" data-target="settings-section-NAME">` to `.settings-nav`: icon, title, subtitle (three items, lowercase-led, no full stop), and the `settings-nav-dot` span.
2. Add `<section class="settings-section" id="settings-section-NAME">` with a `.settings-section-head` (icon, `h3`, one-sentence `p`) and a `.setting-grid`.
3. Add keys for the title, the subtitle and the description to all 20 locales: `settings_section_NAME`, `settings_section_NAME_desc`, `settings_nav_NAME`.
4. Keep the two-column grid at 1000px wide.
5. An advanced tab gets `meld-only` on both its nav button and its section.

Do not add a tab for one or two settings. Put them in the nearest section.

## 6. The map line language

Every line on the map means one thing. The legend (the `?` after the plan's colour chips, `#legend-help`) is drawn by `legendRows(state)` in `src/gui/js/meld-features.js` from the same tables the map draws with: `arnisPieceStyle` colours and dashes in `src/gui/js/piece-style.js`, and the stretch levels `STRETCH_LEVELS` in `meld-features.js`. A swatch cannot disagree with the map. A row shows only while that thing is on the map.

| Line on the map | Meaning | Key |
|---|---|---|
| Yellow dashed box | Your selection while you draw it | `legend_drawing` |
| Yellow solid box | Your selection | `plan_legend_selection` |
| Bright yellow outline | The shape you drew; a cell it touches is built in full | `legend_shape` |
| Yellow dashed box around your shapes | The rectangle One World builds around them (not in cells) | `legend_shape_rect` |
| Blue solid cell, faint fill | Planned for this run | `plan_state` |
| Grey short-dashed cell | Waiting | `run_state_queued` |
| Yellow dashed cell | Running | `run_state_running` |
| Green cell | Done | `run_state_done` |
| Red thick cell | Failed; hover it for the reason | `run_state_failed` |
| Orange dash-dot cell | Stopped; Start resumes it | `run_state_stopped` |
| Violet long-dashed cell | Held: paused by you, waits until resumed | `run_state_held` |
| Hatched grey cell | Skipped: left out of the job | `run_state_omitted` |
| Thick teal cell | Picked with the select tool (the cell menu acts on these) | `legend_picked` |
| Grey filled cell | Outside the outline, not built | `plan_state_excluded` |
| Grey dotted cell | Room to grow: you can build here later | `plan_state_room` |
| Blue dotted cell | Ring (margin) around the selection | `plan_legend_ring` |
| Wide-dotted band | Overlap two neighbouring cells share | `legend_overlap` |
| Yellow dashed line | Past it the map is stretched 5% against real distances | `legend_stretch_5` |
| Red dashed line | Past it the stretch is 10%; start a new world | `legend_stretch_10` |
| Boxes under the selection | Land this world already holds | `legend_held` |

Rules:

- State is never colour alone: a line pattern, a label or an icon says it too (a cell shows a check, a cross or a pause mark; the hover card names the state).
- A new kind of line gets a row in `legendRows`, a swatch from the same table, and a test in `tests/gui/map-legend.mjs`.
- Do not change a colour or dash in `bbox.js` alone. Change `piece-style.js`.
- Cell hovers and other map tooltips use the window's UI font, a state dot and the state word on line one, the size on line two (`bbox.css`, `.map-first .leaflet-tooltip`).
- The grid always draws, by zoom: zoomed in, only the cells inside the visible map (at most 2,000 rectangles, redrawn on `moveend`), each filling from the bottom with its progress; zoomed out past that, the job's outline alone. Labels show only when a cell is large enough on screen.
- The stretch readout (the plan's "Stretch here" line, the 5% and 10% lines and labels on the map, their legend rows) shows only with "Show Map Stretch" in Advanced settings (`meld-stretch-toggle`, off by default). It is the Mercator scale at the selection's latitude against the One World's origin, `cos(origin) / cos(lat) - 1`, the same ratio `one_world.rs` uses.

## 7. Advanced features gating

One switch, `advanced-meld-features-toggle` (Settings > Advanced). The id keeps the project's old name; the label is Advanced features.

- Off (the default): `<body>` has the class `meld-off`, and `body.meld-off .meld-only { display: none }` (`src/gui/css/settings.css`) hides every element marked `meld-only`. `meldFlags` returns no flag, so a build runs as stock Arnis. No plan, no legend, no run controls, no extra setting.
- On: `initMeldFeatures` removes `meld-off`, the elements show, and `meldFlags` adds the flags for the controls that are on.

How to gate something:

- Markup: add `meld-only` to the element (`index.html`: the plan `#job-plan`, `#run-confirm`, `#run-controls`, `#plan-expand-button`, and the advanced setting rows).
- A new setting inherits the gate from `meld-only`; its flag goes through `meldFlags`, never through a second path.
- Elements the script creates (legend, cells, cards) are added only while the switch is on, or sit inside an element that carries `meld-only`.
- A feature that changes the main page for everyone needs the maintainer's say-so. Behind the switch it ships first; `body.map-first` is the one layout change already made default, and Classic stays available.

Test: `node tests/gui/meld-flags.mjs` checks the switch sends no flag when off.

## 8. Strings and languages

- Locale files: `src/gui/locales/*.json`, 20 of them (ar, de, en-US, es, fi, fr-FR, hu, ja, ka-GE, ko, lt, lv, pl, pt-BR, ro, ru, sl, sv, ua, zh-CN). `en-US.json` is the source. `en.json` is a symlink to it; never edit it (on Windows checkouts it is a short text file).
- A new string goes into all 20 files in one commit with the same key. The Rust test `locales_match_en_us_keys` in `src/gui.rs` fails on a missing or extra key.
- Markup carries `data-localize="key"`, with the English text inside as the fallback. Attributes: `data-tooltip-key` (tooltip, English in `data-tooltip` as fallback), `data-localize-title`, `data-localize-aria-label`. The pass in `localizeElement` callers in `main.js` covers `#settings-modal`, `.dialog`, `#run-controls` and `#run-confirm` by selector; a string outside those needs its own selector entry.
- Strings built in JS use `t(key, fallback, { vars })` from `meld-features.js`, with `{name}` placeholders. Re-run the build of such text when the language changes.
- Never build a sentence by gluing translated pieces; give the whole sentence a key with placeholders.
- Numbers, sizes and times go through `toLocaleString` or the existing format helpers, not hand-made separators.
- Romanian is read by hand. Translate meaning, not words; keep tooltips short enough for 240px.
- Wording rules: no "Meld" in user text, no marketing words, a full stop on a sentence, no stop on a label. A legend caption is a label, even a long one. Prose passes the `no-ai-slop` skill.
- Window tests: `for f in tests/gui/*.mjs; do node "$f"; done`.

## 9. Setting previews

A 3D picture can explain a setting that is hard to describe. It shows in a popover when the person hovers or focuses the setting's `?` icon, its control, or a segment (`src/gui/js/preview-popover.js`). Pictures are `src/gui/images/previews3d/<name>.webp`, 720x540, at most 60 KB, with a caption key `preview_<name with _>` in every locale.

Name a picture in the markup, on the `?` icon or the row:

| Attribute | For | Picture name |
|---|---|---|
| `data-preview-on`, `data-preview-off` | a switch | the name given |
| `data-preview-seg` | a segment group, by `data-v` (or `data-preview-key` for another dataset key, `data-preview-group` for the group id) | `<setting>-<value>` |
| `data-preview-sel` | a select, by option value | `<setting>-<value>` |
| `data-preview-num` with `data-preview-steps` | a slider or number box, nearest step | `<setting>-<step>` |

To render one:

1. Add a row `{name, bbox, flags, crop}` to `scripts/previews3d/previews3d.json`. Pick a small bbox where the difference is visible; set both sides of a switch.
2. Build the release binary, then run `python -I scripts/previews3d/previews3d.py [name-prefix] --exe target/release/arnis.exe`. It needs Python with nbtlib, numpy and Pillow, and the network (it fetches live map data, so a picture can differ a little from the committed one). `scripts/previews3d/iso_render_v2.py` is the renderer. Details: `scripts/previews3d/README.md`.
3. Commit the `.webp` and add the caption key to all 20 locales.
4. `node tests/gui/preview-popover.mjs`.

A preview must show what the setting does at its default and at the other value, not a neighbouring setting. If a picture is the same for two values, use one picture and say so in the caption.

## 10. What not to do

- Do not add a colour, a font, a second icon set or an animation library.
- Do not add a build step, a bundler or an npm package. The window is plain files.
- Do not set a default in JS, or add a control without a `SETTINGS` entry.
- Do not hard-code a user-visible string. Do not edit `en.json`. Do not add a key to fewer than 20 locales.
- Do not say "Meld" in user text.
- Do not edit `piece-style.js` colours without checking the legend and `tests/gui/map-legend.mjs`.
- Do not draw a plan over an old one, or accept a plan reply for an older selection.
- Do not signal state with colour alone, and do not drop `prefers-reduced-motion`.
- Do not start a big build on one click. A plan in cells asks first (hold to start).
- Do not change the Classic layout to fix the map-first one; scope the rule with `body.map-first`.
- Do not put a mock-up screenshot in the repo. Use real captures in `docs/images/`.
- Do not hide a failure. An error says what failed and what to try, in the person's language.
