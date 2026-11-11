# Setting previews (3D)

The pictures in `src/gui/images/previews3d/` (shown by the settings page, `src/gui/js/preview-popover.js`) are made here.

    python -I scripts/previews3d/previews3d.py [name-prefix] --exe target/release/arnis.exe

Needs Python with nbtlib, numpy and Pillow. Each entry of `previews3d.json` is `{name, bbox, flags, crop}`; the driver builds
that bbox with the Arnis binary into a temp folder and writes `<name>.webp` (720x540, at most 60 KB). Naming is
`<setting>-<value>.webp`; the page names a picture in `data-preview-on/-off` (switches), `data-preview-seg` (buttons, by `data-v`, or by the dataset key in `data-preview-key`; `data-preview-group` names the button group), `data-preview-sel` (a select, by option value) or `data-preview-num` + `data-preview-steps` (a slider or number box, nearest step). The attributes sit on the setting's ? icon or on its row; the picture shows for the icon, the switch, the select, the slider and each button.
A new picture needs a `preview_<name with _>` caption key in every locale file.

The binary fetches live map data, so a regenerated picture can differ a little from the committed one. After a change to road
markings regenerate just those: `python -I scripts/previews3d/previews3d.py road-detail --exe ...`.

Tests: `node tests/gui/preview-popover.mjs` (from any folder).
