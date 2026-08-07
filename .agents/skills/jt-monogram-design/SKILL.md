---
name: jt-monogram-design
description: Design, revise, implement, or review JT one-stroke monograms and their Figma or animated SVG variants while preserving earlier concepts, semantic intent, geometry, motion, and lineage. Use for JT icon or logo exploration, Figma edits, single-path SVG work, animation specifications, visual acceptance reviews, or recovering JT design context in this repository.
---

# JT Monogram Design

## Load Foundation

Read [references/design-foundation.md](references/design-foundation.md) completely before designing, editing Figma, writing SVG, or reviewing a variant. Treat it as source of truth for first draft, Antfu-inspired mechanism, motion, and acceptance constraints.

## Follow Workflow

1. Inspect canonical Figma file and first-draft node. Record existing sections and frames. If Figma is unavailable, continue from foundation reference and label result unverified.
2. Preserve every historical proposal. Create a new frame for every new concept; never overwrite, delete, rename, or move earlier drafts.
3. Start from letterform. Add at most one secondary reading only when geometry supports it naturally. Prefer motion-only meaning over a literal object when the object weakens `JT`.
4. Draw one open centerline path inside `100 × 100` viewBox. Keep `j` and `t` readable at `16`, `24`, and `64` px. If a dot or loop exists, keep it connected; omitting it is valid for a shared-axis construction. Use round caps, round joins, and safe outer margin.
5. After any glyph-misread rejection, stop at a black-white form gate. Reject `τ`, `h`, `π`, `f`, or single-letter readings before adding meaning, pressure, color, or motion.
6. Build pressure silhouette as alpha mask around an accepted centerline. Animate centerline with `stroke-dasharray` and `stroke-dashoffset`; let mask control visible ink body.
7. Add editable centerline, pressure mask, static icon, meaning annotation, drawing order, motion timeline, and size checks to new Figma frame. For an unapproved form-gate candidate, add only positive/negative form, recognition rules, and optical sizes.
8. Verify resulting Figma node after write. Report URL, node ID, approval status, complete `d`, size checks, and confirmation first draft stayed untouched. Report timing and reduced-motion behavior only after motion exists.

## Keep Design Honest

- Borrow Antfu logo's one-stroke SVG mechanism, handmade energy, and reveal rhythm. Do not trace, transform, or copy `af` contour, anchors, proportions, or letter construction.
- Prefer form-first construction. Use one strong metaphor at most; use none when it needs explanation or makes the silhouette worse. Avoid hidden-detail overload, extra badges, and multiple paths.
- Never rescue weak geometry with copy. Meaning follows accepted form; it cannot establish the letter reading.
- Make every visual bend serve letter recognition, stroke continuity, or stated meaning.
- Distinguish Figma keyframe presentation from runtime SVG animation. Figma documents motion; exported SVG/CSS performs it.
- Preserve exact historical path in foundation reference even after later variants supersede it.

## Name Variants

Create section `JT Monogram / Explorations` when absent. Name frames `V<n> — <meaning>`. Keep each variant self-contained. Reuse presentation scaffolding only; redraw centerline for new concept.

## Validate

Run every item in foundation acceptance checklist. Do not call design complete while first-draft preservation, single-path continuity, semantic mapping, small-size recognition, mask coverage, motion reset, or `prefers-reduced-motion` remains unchecked.
