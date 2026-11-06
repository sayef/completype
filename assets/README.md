# Brand assets

| File | Use |
|---|---|
| `wordmark-light.svg`, `wordmark-dark.svg` | The name, for light and dark backgrounds |
| `banner-light.svg`, `banner-dark.svg` | The name and tagline on their own ground, as at the top of the README |
| `logo.svg` | The mark: favicons, avatars, the docs tab icon |
| `social-preview.svg`, `social-preview.png` | 1280 × 640 repository preview for GitHub and link cards |
| `demo.svg` | Animated demo of real completions |

## The idea

The wordmark is the name being completed. `comple` is what was typed; a caret follows it; `type` is the
completion, selected the way an address bar selects the rest of a URL it has filled in for you. The mark
is the same idea at favicon size: a `c` and the block of its completion on an ink tile.

## Colour

| Name | Hex | Job |
|---|---|---|
| Ink | `#121210` | Text on light grounds, the dark ground, the logo tile, text inside the selection |
| Paper | `#F5F4EE` | The light ground, text on dark grounds |
| Selection | `#D4F13A` | The completion block, the caret in the demo, small markers. Always behind ink, never as text on a light ground |
| Selection ink | `#456B00` | Links and the accent on light pages, where the selection colour itself would not read (6:1 on white) |
| Muted | `#5E5D56` / `#A3A29A` | Taglines and secondary text, on light and on dark |

The selection colour is a fill. On dark pages it may also colour links and small marks; on light pages
use selection ink. Ink on selection is 14.7:1.

## Type

- [Martian Mono](https://github.com/evilmartians/mono), Bold at width 100, for the name, and Regular for
  code, queries and labels. A monospaced name keeps every letter on the same grid as the caret and the
  selection.
- [Instrument Sans](https://github.com/Instrument/instrument-sans), Medium and Regular, for taglines,
  documentation text and the demo's results.

Both are under the SIL Open Font License. Text in the SVGs is converted to outlines, so the files need no
fonts.

## Rules

- Keep the selection block and the caret; the name in one colour is not the wordmark.
- Use the banner with its own ground on any page; use a bare wordmark only on a matching background.
- Do not recolour the selection, set the name in another face, or add effects.

## Regenerating

```sh
uv run --no-project --with fonttools --with cairosvg python assets/src/build.py <font dir>
uv run --no-project --with fonttools python assets/src/demo.py <font dir>
```

The font directory holds the variable fonts `MartianMono.ttf` and `InstrumentSans.ttf` from
[google/fonts](https://github.com/google/fonts/tree/main/ofl). `build.py` also writes
`social-preview.png` (when `cairosvg` is installed) and refreshes the copies in `docs/assets/`. The demo
reads its scenes from `src/demo.json`; `python assets/src/demo_data.py` recomputes them with completype.
