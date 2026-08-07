# JT Monogram Design Foundation

## Canonical Artifact

- Figma file: <https://www.figma.com/design/jKbvxEvAJqa56srBQIWEEk>
- File key: `jKbvxEvAJqa56srBQIWEEk`
- Canonical first-draft main frame: node `3:2`
- Exploration section: node `10:2`
- Second-draft frame: node `10:3`
- Third-draft frame: node `16:2`
- Canvas: `100 × 100`

Keep node `3:2` intact. Put each later proposal in new frame and, when useful, new section. Never repurpose first-draft frame as latest variant.

## Antfu-Inspired Method

Borrow construction method, not `af` shape:

1. Draw single open centerline in actual pen order.
2. Use centerline as animation driver with `stroke-dasharray` and `stroke-dashoffset`.
3. Clip driver through `mask-type="alpha"` pressure silhouette. White alpha reveals ink; transparent area hides it.
4. Use round line caps and joins. Taper silhouette at entry and exit instead of adding separate cap shapes.
5. Keep static complete mark when `prefers-reduced-motion: reduce`.

Normalize animation with `pathLength="1"` when implementing SVG. Driver width must cover widest pressure silhouette; mask supplies final thickness. Do not close path with `Z`.

## First Draft Meaning

Primary reading: connected lowercase `j` and `t`.

Secondary reading: turning setbacks into forward motion.

- Small connected opening loop acts as spark, intent, and `j` dot.
- Long descent acknowledges detour rather than hiding it.
- Broad bottom hook converts downward energy into rising `t` stem.
- Cross sweep acts as horizon or decision line.
- Final rising tail signals continuation, not terminal stop.

Meaning depends on stroke sequence: begin, descend, turn, rise, cross, continue.

## First Draft Geometry

- Left letter mass centers near `x=34`; right letter mass near `x=63`.
- Visible gesture spans roughly `x=17…81`, `y=8…86`; retain surrounding quiet space.
- Tight opening loop contrasts with broad lower recovery arc.
- Long rising curve supplies shared transition from `j` hook into `t`.
- Horizontal sweep around `y=34…38` makes `t` legible.
- Exit curve ends higher than lowest point, preserving upward momentum.

Complete single open path:

```svg
d="M34 16 C28 12 26 19 31 22 C35 24 39 20 36 16 C34 28 33 44 31 61 C30 72 25 79 17 77 C22 86 38 83 48 70 C55 59 59 43 61 24 C62 14 65 8 68 12 C71 17 68 28 63 36 C56 34 49 35 44 38 C54 35 67 34 78 34 C70 37 65 40 63 47 C60 57 59 69 63 76 C67 83 75 79 81 71"
```

Drawing order:

1. `M34 16 … 36 16`: make connected opening loop; never detach dot.
2. `C34 28 … 17 77`: descend through `j` stem into lowest turn.
3. `C22 86 … 61 24`: rebound through hook into shared rising `t` stem.
4. `C62 14 … 63 36`: form narrow crown loop and return to crossing zone.
5. `C56 34 … 78 34`: sweep left, then right, creating `t` crossbar.
6. `C70 37 … 81 71`: return below bar, descend, then finish with rising exit.

## Second Draft: Seed, Root, Sprout

Primary reading: connected lowercase `j` and `t`.

Secondary reading: an idea survives descent, takes root, then grows outward.

- Opening loop is seed and connected `j` dot.
- Long `j` descent places seed below the surface.
- Broad lower sweep is root and resilience.
- Shared rising stroke becomes stem and growth.
- Left-to-right cross sweep is leaf and connection.
- Rising open exit means work continues.

Meaning follows pen order: seed, descend, root, rise, leaf, continue. Keep this as one metaphor; do not add literal plant shapes.

Complete single open path:

```svg
d="M29 22 C24 18 26 12 32 13 C38 14 39 21 35 25 C32 28 28 25 30 23 C32 39 30 55 27 69 C25 78 20 82 14 78 C23 91 37 90 48 80 C60 69 62 47 66 26 C68 15 72 10 76 14 C82 20 77 35 68 45 C61 51 52 50 45 48 C56 42 72 41 88 44 C79 49 73 57 72 68 C70 79 72 86 78 87 C84 88 89 82 93 76"
```

Figma nodes:

- Exploration section: `10:2`
- Proposal frame: `10:3`
- Variable-pressure master: `11:3`
- Semantic construction grid: `12:3`
- Growth sequence: `10:6`
- Motion and optical-size spec: `10:7`

Pressure targets at `100 × 100`: light base `1.8`; seed `2.7`; `j` descent `4.2`; root `4.5`; rising stem `3.2`; crown `2.8`; leaf `2.3`; right descent `4`. Use a `7`-unit animation driver inside the alpha mask.

Ten-second loop: write `0–40%`, hold `40–85%`, return `85–100%`. Reduced motion shows the complete static mark. Optical-size compensation: `5.5`, `4.8`, `4.4`, and `4` at `16`, `24`, `32`, and `64` px.

## Third Draft: Command Flag

Primary reading: a compact `J/T` monogram with one shared axis instead of two side-by-side lowercase letters.

Secondary reading: CLI command flag, execution, and returned output.

- Left horizontal entry is prompt and the left arm of `T`.
- Pennant is a literal flag, the CLI option meaning, and the right arm of `T`.
- Central pole is the shared `J/T` axis and the execution stroke.
- Bottom turn is the `J` hook and returned output.
- Open terminal is the next prompt waiting.

This is intentionally unrelated to first- and second-draft geometry: no connected dot loop, no left `j` descent, no broad recovery arc joining a separate right `t`, and no plant metaphor.

Complete single open path:

```svg
d="M18 37 C30 37 42 37 53 37 C64 36 75 32 85 27 C75 22 64 18 53 18 C53 34 53 50 53 66 C53 77 48 84 39 86 C27 88 18 80 18 68"
```

Drawing order: prompt, flag lower edge, flag upper edge, shared execution axis, return hook. The path contains one `M`, six `C` commands, no `Z`, and one open terminal.

Figma nodes:

- Exploration section: `10:2`
- Proposal frame: `16:2`
- Variable-pressure master: `18:2`
- Semantic construction grid: `19:2`
- Command sequence: `16:50`
- Motion and optical-size spec: `16:120`
- Optical-size row: `16:128`

Pressure targets at `100 × 100`: light base `1.8`; prompt `3.2`; flag lower edge `2.6`; flag upper edge `2.8`; shared axis `4.5`; return bend `5.2`; animation driver `7.5`.

Ten-second loop: write `0–42%`, hold `42–82%`, return/reset `82–100%`. Internal write order uses prompt, flag, execute, and return stages. Reduced motion shows the complete static command flag.

At `16` and `24` px, enlarge the pennant negative space and reduce pressure. Current compensated paths:

```svg
<!-- 16 px -->
d="M12 39 C27 39 42 39 54 39 C68 38 81 32 90 26 C79 19 66 14 54 14 C54 31 54 49 54 66 C54 77 49 83 40 85 C29 87 20 80 19 70"

<!-- 24 px -->
d="M15 38 C29 38 42 38 54 38 C67 37 79 32 88 27 C78 21 66 16 54 16 C54 32 54 49 54 66 C54 77 49 84 40 86 C29 88 20 80 19 69"
```

Use stroke widths `4.2`, `4.4`, `4.1`, and `4` at `16`, `24`, `32`, and `64` px. Figma contains motion storyboard and timing specification; runtime SVG/CSS performs the actual animation.

## Fourth Draft: Shared Axis, Form First

Status: rejected. Preserve the frame as history; never present it as an active direction.

Rejection reason: the shared axis removes the independent `J` structure. The first read is Greek `τ`, not `JT`. Refining stroke weight, copy, semantics, or animation cannot repair this topology.

Primary reading: `J` and `T` superimposed on one curved axis. The opening sweep is the `T` crossbar; its low fold returns into the same stem that resolves as the `J` hook.

Secondary reading is motion only: turn and continue. There is no literal object, hidden badge, detached dot, or semantic shape to explain. This draft records the correction after the third draft: do not force meaning onto weak geometry.

Complete single open path:

```svg
d="M14 31 C34 25 58 25 86 31 C75 28 64 27 54 29 C58 40 58 52 56 64 C53 78 47 86 37 88 C27 90 18 83 18 73"
```

Drawing order:

1. Sweep left to right to establish the `T` crossbar.
2. Fold back close to the same contour so the return reads as pen pressure, not a separate object.
3. Drop through the shared curved axis.
4. Add pressure through the lower turn.
5. Finish with an open `J` terminal.

The path contains one `M`, five `C` commands, no `Z`, and one open terminal.

Figma nodes:

- Exploration section: `10:2`
- Pure-form comparison strip: `25:2`
- Proposal frame: `27:2`
- Variable-pressure master: `30:2`
- Centerline and anchor grid: `30:10`
- Draw sequence: `27:10`
- Motion and optical-size spec: `27:11`

Pressure targets at `100 × 100`: light base `1.35`; opening sweep `2.5`; low fold `1.8`; shared axis `3.6`; lower turn `4.5`; open terminal `3.5`.

Runtime loop: `2.6 s`, draw `0–48%`, hold `48–82%`, unwrite `82–100%`, `ease-in-out`. Reduced motion shows the complete static mark.

Optical-size centerline widths: `6.2`, `5.4`, `4.8`, and `4` at `16`, `24`, `32`, and `64` px. At small sizes, preserve the low fold as a compact thickened crossbar; do not reopen it into the taller crown used by rejected candidate A.

## Fifth Draft: Reverse t, Form Gate

Status: active black-white candidate, not user-approved. Do not add semantics, pressure mask, or animation until the user accepts the letterform.

Primary reading: capital `J` on the left and independent looped lowercase `t` on the right.

- The complete left descent and open lower hook establish `J`.
- The right ascender owns a separate axis; never merge it with the `J` stem.
- One long cross stroke joins the letters and crosses the `t` once.
- Small reversals at both ends retain handmade energy without adding an object metaphor.
- Drawing runs right-to-left through the `t` and crossbar, then resolves the `J`. Reverse drawing order is deliberate.

Complete single open path:

```svg
d="M86 64 C82 72 76 72 73 66 C69 57 71 46 72 35 C73 28 77 23 82 24 C87 25 87 30 83 33 C77 38 68 39 58 37 C46 35 34 32 22 29 C15 27 12 31 16 34 C23 36 30 38 33 45 C37 56 33 69 32 78 C31 86 24 89 17 86 C12 84 10 78 13 73"
```

The path contains one `M`, eleven `C` commands, no `Z`, and one open terminal.

Figma nodes:

- Exploration section: `10:2`
- Form-gate board: `35:2`
- Positive master: `36:3`
- Negative master: `36:6`
- Recognition panel: `35:9`
- Optical-size marks: `36:19`, `36:22`, `36:25`, `36:28`

Form gate:

- Pass only when the unannotated mark reads `JT`.
- Reject `τ`, `h`, `π`, `f`, or single-`J` readings.
- Check positive and negative forms.
- Check `16`, `24`, `32`, and `64` px before pressure work.
- Keep motion out of this board. Add it in a new approved-variant frame, not by overwriting this gate.

## Pressure Mask

Shape pressure after centerline works:

- Keep upstrokes and tight loops light.
- Thicken long descents and lower turning arc.
- Ease thickness before tight direction reversals; avoid blobs at opening loop and crossbar.
- Taper first and last 5–8% of path.
- Keep pressure changes gradual across adjacent cubic segments.
- At self-crossings, preserve enough negative space to read stroke order at `24` px.

Use centerline only for timing. Use mask silhouette for handmade width variation. Test mask coverage with solid contrasting driver stroke before applying final color.

## Ten-Second Loop

Default rhythm:

- `0–10%` (`0–1 s`): blank pause.
- `10–45%` (`1–4.5 s`): write full path in pen order.
- `45–70%` (`4.5–7 s`): hold complete mark.
- `70–90%` (`7–9 s`): remove or fade mark in same directional flow.
- `90–100%` (`9–10 s`): blank reset.

Use eased entry and exit without stopping at every anchor. Under `prefers-reduced-motion: reduce`, disable animation and show complete path with zero dash offset.

## Variant Workflow

1. Start with `JT` recognition and continuous motion. Pick at most one compatible metaphor; choose no literal metaphor when it damages the silhouette.
2. State letter roles before geometry. Add secondary meaning only when visible without explanatory copy.
3. Draw centerline first. Remove any bend lacking letter, continuity, or meaning function.
4. Confirm `JT` silhouette without annotation. Then refine pressure mask.
5. After a misread rejection, publish a black-white form-gate frame first. Continue only after acceptance.
6. Put proposal in new `V<n> — <meaning>` frame under `JT Monogram / Explorations`.
7. Include editable vectors and exact SVG path. Keep comparison thumbnail of first draft; link to node `3:2` instead of duplicating or modifying it.
8. Add four motion states only for an accepted form: blank, partial write, complete hold, partial exit.

## Acceptance Checklist

- [ ] Figma file key and target node verified before editing.
- [ ] First-draft node `3:2` unchanged after editing.
- [ ] New proposal lives in new named frame or section.
- [ ] Mark reads as `JT` at `16`, `24`, and `64` px.
- [ ] Mark does not collapse into `τ`, `h`, `π`, `f`, or one isolated letter.
- [ ] Any secondary meaning maps to visible gestures; no post-hoc explanation is needed.
- [ ] Centerline is one open path with no disconnected dot and no `Z`.
- [ ] Complete `d` is stored with proposal.
- [ ] Outer margin and negative space survive pressure mask.
- [ ] Driver fully covers mask at all bends and crossings.
- [ ] Caps, joins, start, and finish look intentional.
- [ ] Ten-second write, hold, exit, and reset loop has no flash.
- [ ] Reduced-motion mode shows complete static mark.
- [ ] Geometry does not copy or trace Antfu `af` outline.
