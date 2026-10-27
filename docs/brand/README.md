# Sych — owl-head brand pack (amber edition)

The mascot uses a rounded **little owl head**, without ear tufts, body or detached wings. Amber-yellow eyes and rust-orange beak nod to Rust.

## Contents

- `symbol-transparent.png` — transparent standalone mascot art
- `icon-transparent-{16,32,48,64,128,256,512,1024}.png` — square padded transparent icons
- `avatar-{night,bone,rust,amber}-*.png` — square avatars for GitHub/other services
- `lockup-{dark,light,transparent-light,transparent-dark}.png` — horizontal lockups
- `readme-banner-2400x800.png` — GitHub README hero
- `social-preview-1280x640.png` — GitHub social preview
- `brand-guide.png` — overview of the unified identity

## Colors

| Name | HEX |
|---|---|
| Night | `#0B1016` |
| Graphite | `#101923` |
| Bone | `#F6F4EE` |
| Amber | `#FFC83D` |
| Rust Orange | `#F97416` |
| Slate | `#9CA3AF` |

Fonts used for new typography: Inter Display and Inter.

## Usage

For the `README.md` header:

```markdown
<p align="center">
  <img src="docs/brand/readme-banner-2400x800.png" alt="Sych — deterministic code review" width="100%" />
</p>
```

For the GitHub App profile, start with `avatar-night-1024.png`.

**Format note:** The original generated mascot is raster artwork. The assets are PNG (not native vector/editable SVG). Avoid claiming they are editable vectors or using automated vector tracing without QA.
