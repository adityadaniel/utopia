# Icons

The 16 px line icons of DESIGN.md §2.5, from [Lucide](https://lucide.dev) (ISC licence,
`LICENSE`), embedded in `utopia-ui` through `src/icons.rs` so a signed bundle needs no
resource lookup.

Each file keeps Lucide's 24-unit viewBox and draws with a **3-unit** stroke: rendered at
16 px that is the 2 px stroke the spec asks for. The SVGs are monochrome on purpose —
gpui turns the drawing into an alpha mask and tints it with the element's `text-color`
(`--ink-3` by default), so one file serves both themes.
