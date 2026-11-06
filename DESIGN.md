# Design and brand

The design system for completype's brand assets, README and documentation site. Read it before changing
anything a reader sees: `assets/`, `docs/stylesheets/`, `docs/javascripts/`, `overrides/` or the landing page.
Writing has its own guide, [Writing the docs](https://completype.pages.dev/writing/).

## The idea

completype completes what people type, and its look is that moment: what was typed, a caret, and the rest
of the word suggested as a selection, the way an address bar selects the part of a URL it has filled in for
you. The wordmark is `comple`, a caret, and `type` on a block of the selection colour. Everything else is
paper and ink, so that one block is what people remember.

| Surface | Mode | Bold | Motion | Density |
|---|---|---|---|---|
| Landing page | Persuade | 6 | 4: the headline is typed and completed, once per claim | 3 |
| Documentation pages | Read | 2 | 1: state changes only | 5 |
| README | Read | 3 | the animated demo only | 5 |

## Colour

```yaml
colors:            # light            dark
  ground:          ["#FAF9F5",       "#121210"]   # the page: warm paper, or ink
  step:            ["#F1F0E9",       "#1C1B18"]   # code, admonitions, fields, cards: one tone step from ground
  line:            ["#E3E1D8",       "#2D2C27"]   # hairlines: table rows, header, card outlines
  ink:             ["#121210",       "#F5F4EE"]   # text, at 92 % for body copy
  ink-secondary:   ["#5E5D56",       "#A3A29A"]   # supporting text, >= 4.5:1 on ground and step
  selection:       "#D4F13A"                      # the wordmark's block, text selection, the current page
  on-selection:    "#121210"                      # always ink, in both themes (14.7:1)
  link:            ["#456B00",       "#D4F13A"]   # selection ink on light (about 6:1); the selection itself on dark
  mark:            ["#D4F13A at 55 %", "#D4F13A at 22 %"]   # the current page in the sidebar, highlighted lines
```

Rules:

- The selection colour is a fill behind ink. On a light ground it never colours text or thin lines, where it
  cannot be read; use `link` there.
- One accent. No second hue, no gradients, no tinted shadows. The demo's match-kind badges are the only
  other colours, and they appear only inside the demo.
- Both themes are first-class. Every token above has a value for each, and `color-scheme` and
  `theme-color` follow the theme.

## Typography

```yaml
typography:
  text: { family: "Instrument Sans", weights: [400, 500, 600], fallback: "ui-sans-serif, system-ui, sans-serif" }
  mono: { family: "Martian Mono", widths: [87.5, 100], weights: [400, 500, 700], fallback: "ui-monospace, Menlo, monospace" }
  scale:                     # docs pages, in rem of the 20px root Material sets
    display:  { size: "clamp(2.2rem, 4.6vw, 3.3rem)", weight: 600, lineHeight: 1.06, tracking: "-0.035em" }  # landing only
    title:    { size: 1.95rem, weight: 600, lineHeight: 1.12, tracking: "-0.03em" }
    heading:  { size: 1.3rem,  weight: 600, tracking: "-0.02em" }
    body:     { size: 0.82rem, weight: 400, lineHeight: 1.68 }
    small:    { size: 0.74rem, weight: 400 }                      # tables, admonitions
    label:    { size: 0.6rem,  family: mono, width: 87.5 }         # sidebar sections, figure labels
  numerals: tabular-nums everywhere
```

Rules:

- Martian Mono is for code, queries, file names and short labels, set at width 87.5 so long lines fit. The
  wordmark uses it Bold at width 100. Nothing else is set in mono.
- Labels are lower or sentence case. No tracked capitals, no eyebrows above headings.
- Fonts are served from the site itself: the Material `privacy` plugin downloads them at build time.

## Shape and depth

```yaml
radius: { surface: 4px, control: 4px, chip: 3px }   # square, like the selection block; the demo keeps its own
elevation:
  rest: "one tone step (ground to step) and a 1px line; no shadow"
  floating: "only the demo image on the landing page casts a shadow"
spacing: { base: 4px, section: 88px on the landing page }
```

## Components

- **Header.** The page's own ground with a hairline below, the wordmark for that theme (`wordmark-light.svg`
  on light, `wordmark-dark.svg` on dark), never a coloured bar.
- **Sidebar.** Section titles as mono labels. The current page sits on the `mark` tint, like selected text;
  the current heading in the table of contents has a 2px selection rule.
- **Tables.** No grid: a rule under the header and hairlines between rows. Numbers are tabular.
- **Code.** A step-tone block with a 1px line and square corners; highlighted lines use the `mark` tint.
- **Admonitions.** A step-tone block with a 2px rule on the left in the admonition's colour.
- **Cards.** Hairline and square; on hover the outline darkens and a 3px selection rule appears on the left.
- **Buttons** (landing page). Solid ink, or an outline; hover turns the solid one into the selection colour.
  Labels name the action: "Get started", "View on GitHub", "Read the concepts".
- **Copy page.** Every page has a button that copies its Markdown, snippets included, for pasting into an
  editor or an agent.
- **Diagrams.** Mermaid on docs pages; inline SVG on the landing page, drawn with the tokens above so it
  follows the theme.

## Landing page

The landing page (`overrides/home.html`, `docs/stylesheets/home.css`) makes one argument per section, each
with its proof:

1. **Autocompletion as a library.** The headline types itself and is completed in the selection colour,
   cycling through five claims that the benchmarks support. The animated demo of real results sits beside it.
2. **The numbers.** Four figures from [Benchmarks](https://completype.pages.dev/benchmarks/), with a note
   that they come from one laptop.
3. **Every kind of match.** Six queries and their real top completions from the demo index.
4. **The database is a bucket.** Three properties of the storage protocol and a diagram of it.
5. **One catalogue, many views.** The layer stack and the real output of a search through it.
6. **A few lines of code**, in Python and in Rust, then common questions and one call to action.

Every number and result on it comes from the README, the guides or the benchmark results. When those
change, the landing page changes with them.

## Motion

The landing headline types at about 50 to 90 ms per character, completes, holds for 2.6 s and moves to the
next claim. The caret blinks. With `prefers-reduced-motion`, the first claim stands still with its
completion shown and the caret does not blink. Docs pages animate only state changes, in 120 ms.

## Brand assets

Assets are generated, never drawn by hand. `assets/src/build.py` writes the wordmarks, banners, logo and
social preview with text converted to outlines, so they need no fonts to view; `assets/src/demo.py` writes
the animated demo. Both, and the rules for using the assets, are described in
[`assets/README.md`](https://github.com/sayef/completype/blob/main/assets/README.md).

- Banners carry their own rounded ground, so they read on any page theme. The README uses them with a
  `<picture>` element that picks the one for the reader's theme.
- The wordmark is never set in another face, recoloured, or shown without its selection block.
- The README is also the crates.io and PyPI page, so every image and link in it is an absolute URL.

## Checking a change

1. Build the docs strictly: `mkdocs build --strict` must finish with no warnings.
2. Look at the landing page and one guide in both themes, at a phone width (375 px) and a laptop width,
   with a headless browser or by hand. Nothing scrolls sideways; headlines stay within two lines.
3. Check contrast for any new colour pair: body text 4.5:1, large text 3:1.
4. Regenerate the assets when `build.py` changes, and look at the PNG preview of each.
