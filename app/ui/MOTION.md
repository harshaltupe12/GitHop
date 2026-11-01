# GitHop motion spec

Motion in GitHop shows cause and effect: what opened, what moved, what changed.
It never decorates. [Design §10](../../Design.md#10-interaction-and-motion) is
binding. This file says how §10 is applied, component by component.

## Rules

1. **One easing.** Use `--ease` (`cubic-bezier(0.2, 0, 0, 1)`, a strong ease-out)
   everywhere. Spinners are the only exception: they turn at a constant (linear) rate.
   No bounce, no spring, no overshoot.
2. **Short.** Enter animations take 160 ms (`--dur-base`) and exits take 120 ms
   (`--dur-exit`). A screen change takes 180 ms (`--dur-screen`). Hover and press
   take 120 ms (`--dur-fast`). Nothing in the app UI runs longer than 180 ms.
3. **Animate only transform and opacity.** Use the individual `scale` and
   `translate` properties. They combine with Tailwind's `transform` utilities instead
   of overwriting them, and the compositor runs them. There are two documented
   exceptions, both on one tiny element: the width of the onboarding progress dot,
   and the width of the segmented thumb when its segments differ in width.
4. **Never start from nothing.** A layer that appears starts at `--enter-scale`
   (0.97) or `--enter-shift` (0.5 rem), never at `scale(0)`.
5. **Focus is instant.** Never transition `outline` or `outline-color`. A
   keyboard user sees the ring on the same frame as the focus.
6. **Mount is not change.** A list or table already on screen when a view first
   shows never animates in row by row. Only items that arrive later animate, and
   only up to 24 at a time.
7. **Interruptible.** Movement uses CSS transitions (the indicator, toggle knob and
   toast snap-back) or additive WAAPI (toast reflow), so a second change takes over
   from where the element is now. Enter and exit keyframes are only used on layers
   that can't be re-triggered mid-flight.
8. **Reduced motion.** `prefers-reduced-motion: reduce` covers Windows "Animation
   effects" being off. Under it, every transition and keyframe runs in 0 ms (the
   global block in `index.css`) and the press scale is off. The JS helpers skip
   exit phases and FLIP, so layers unmount at once. The one exception is spinners:
   they keep turning, at half speed. A frozen spinner looks like a hang, and Windows
   also keeps its own busy cursor turning when animations are off.
9. **Forced colours** (Windows contrast themes) aren't affected. The segmented
   thumb uses `Highlight` and `HighlightText`, and the selected nav item keeps its
   outline.

## Tokens (`src/styles/tokens.css`)

| Token           | Value                        | Use                                                                                      |
| --------------- | ---------------------------- | ---------------------------------------------------------------------------------------- |
| `--ease`        | `cubic-bezier(0.2, 0, 0, 1)` | All UI motion (Design §10)                                                               |
| `--dur-fast`    | 120 ms                       | Hover, press, colour changes, tooltip, chip pop                                          |
| `--dur-base`    | 160 ms                       | Layer enter (dialog, menu, toast, tray), indicator glide, toggle knob, row and pill fade |
| `--dur-screen`  | 180 ms                       | Screen change, onboarding step                                                           |
| `--dur-exit`    | 120 ms                       | Layer exit (dialog, menu, toast)                                                         |
| `--dur-spin`    | 800 ms                       | One spinner turn                                                                         |
| `--press-scale` | 0.98                         | `:active` on small controls                                                              |
| `--enter-scale` | 0.97                         | Where dialogs, menus, tooltips and chips start                                           |
| `--enter-shift` | 0.5 rem                      | How far toasts, the tray menu and onboarding steps travel                                |

Tailwind exposes the durations as `duration-fast`, `duration-base`,
`duration-screen` and `duration-exit`.

## Utilities and hooks

| Name                                         | Where          | What                                                                                                     |
| -------------------------------------------- | -------------- | -------------------------------------------------------------------------------------------------------- |
| `.transition-ui`                             | index.css      | Colour, background, border, opacity and box-shadow over 120 ms. No outline.                              |
| `.press`                                     | index.css      | `.transition-ui` plus `scale: 0.98` on `:active` (skipped when disabled or `aria-disabled`)              |
| `.fade-in`                                   | index.css      | Screen fade-in, 180 ms                                                                                   |
| `.appear`                                    | index.css      | Opacity fade-in, 160 ms (banner, changed pill, new row)                                                  |
| `.pop-in`                                    | index.css      | Opacity and 0.97 scale in, 120 ms (new chip, colour check)                                               |
| `.spin`                                      | index.css      | Spinner: fades in, then turns                                                                            |
| `usePresence(open)`                          | lib/motion.tsx | Keeps a layer mounted through its exit. Returns `data-state` plus `onAnimationEnd`, with a safety timer. |
| `useArrivals(keys)`                          | lib/motion.tsx | Picks out the keys that arrived after first render (max 24 per batch)                                    |
| `useChangeCount(v)`                          | lib/motion.tsx | Change counter used as a `key` to replay a fade when a value changes                                     |
| `useSlidingIndicator` + `<SlidingIndicator>` | lib/motion.tsx | Measures the active item with `offset*` (re-measured on resize) for a gliding highlight                  |
| `useFlip(ref, dep)`                          | lib/motion.tsx | Reflows a small stack with FLIP (additive WAAPI `translate`, one layout read per change)                 |
| `useSwipeDismiss(fn)`                        | lib/motion.tsx | Drag right to dismiss. Distance ≥ 56 px or speed > 0.11 px/ms; the other way is damped.                  |
| `tooltipGroup`                               | lib/motion.tsx | Skip-delay: after one tooltip, the next opens at once (300 ms window)                                    |

`motionEnabled()` is false when `matchMedia` is missing (jsdom) or reduced
motion is on. Unit tests therefore get the instant path by default. To test
motion, stub `matchMedia`, as `src/lib/__tests__/motion.test.tsx` does.

## Per component

| Component     | Enter                                                                                   | Exit / change                                                                           | Notes                                                                                                                                                 |
| ------------- | --------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| Button        | —                                                                                       | Press: `scale 0.98`, 120 ms. Hover and press colours 120 ms.                            | The loading spinner fades into the icon's 16 px slot, so the width doesn't change when the button has an icon.                                        |
| Nav item      | —                                                                                       | The selected background glides between items (`transform`, 160 ms)                      | The item paints its own background until the indicator has measured. Ctrl+1–4 glides too: it's short, and it shows where you went.                    |
| Screen change | Opacity 0 → 1, 180 ms                                                                   | —                                                                                       | Keyed on the screen, with no exit, so there's no layout shift and no double render                                                                    |
| Dialog        | Scrim fades in. Panel goes from opacity 0 and scale 0.97 to 1, 160 ms, from the centre. | Reverse, 120 ms. Stays mounted and keeps showing its last content until `animationend`. | `aria-hidden` and `pointer-events: none` while closing. Focus returns to the opener at once. No shake on a wrong typed confirmation (§10: no bounce). |
| Menu          | Opacity plus scale 0.97 → 1, 160 ms, from the corner touching the trigger               | 120 ms fade and scale out                                                               | `origin-top-right` and similar, chosen from `align` × `placement`                                                                                     |
| Tooltip       | 500 ms delay, then opacity plus scale 0.97 → 1, 120 ms, from the trigger side           | Hides instantly                                                                         | A neighbour within 300 ms opens with no delay and no animation                                                                                        |
| Toast         | Rises 0.5 rem and fades in, 160 ms                                                      | Fades and scales out, 120 ms. A swiped toast slides off to the right.                   | The others reflow with FLIP. Auto-dismiss is unchanged (4 s; errors stay).                                                                            |
| Pill          | —                                                                                       | Status change fades the new pill in, 160 ms                                             | Never on first render                                                                                                                                 |
| Table row     | —                                                                                       | Rows added later fade in, 160 ms                                                        | Never on mount, never more than 24 at once, never on removal                                                                                          |
| Toggle        | —                                                                                       | Knob slides 1 rem (`translate`), track colour changes, 160 ms                           | Press scale on the track                                                                                                                              |
| Segmented     | —                                                                                       | Aqua thumb glides (`transform` + `width`), 160 ms                                       | The selected text colour changes over 120 ms                                                                                                          |
| Colour picker | —                                                                                       | Ring fades (box-shadow, 120 ms), check pops in                                          | Press scale on swatches                                                                                                                               |
| Chip input    | Added chip pops in, 120 ms                                                              | Removed instantly                                                                       | Chips already on a profile don't animate                                                                                                              |
| Banner        | Fades in, 160 ms                                                                        | —                                                                                       | Appears with its content; it never animates away                                                                                                      |
| Onboarding    | Step slides 0.5 rem from the direction of travel and fades in, 180 ms                   | —                                                                                       | The active progress dot widens, 160 ms                                                                                                                |
| Tray menu     | Rises 0.5 rem from the taskbar side and fades in, 160 ms, every time it is shown        | Goes transparent instantly on blur, Esc or a choice                                     | So it doesn't flash the stale frame when it's shown again                                                                                             |

## Deliberately not animated

- **Focus rings.** They must be instant.
- **Destructive results** (wipe, delete, fixed commits). Content is never
  animated away before or after confirmation (§10).
- **Table row removal and Fix rows after a fix.** Removal stays instant: a fading
  row would read as "still there".
- **Table and list mount, and big rescans.** A table that is already on screen
  never animates in.
- **Height and size of anything else.** Inserted content pushes layout
  instantly. Animating height needs per-frame layout work, and §10 asks for
  function, not flourish.
- **Typed-confirmation shake.** It's a bounce, which §10 rules out. The disabled
  Danger button already explains the state.
- **Keyboard-repeat actions** (arrow keys in menus, the sidebar, segmented
  controls and swatches). The focus moves instantly. Only the indicator glides.
- **Skeletons.** There aren't any. Loading uses a spinner and text (§8.7).
- **Scan progress bar.** The scan reports a found count, not a total, so a
  determinate bar would be dishonest. The spinner and live count stay.

## Do / don't

- Do use `--ease` and the duration tokens. Don't hard-code `ms` values or easing curves.
- Do use `scale` and `translate`. Don't animate `top`, `left`, `width`,
  `height`, `margin` or `padding` (except the two exceptions above).
- Do put new layers behind `usePresence` so they can play their exit. Don't
  unmount a layer mid-animation.
- Do check every new animation with `prefers-reduced-motion: reduce` and in a
  Windows contrast theme.
- Don't add an animation library. Everything here is CSS plus about 250 lines of hooks.
- Don't animate something the user does dozens of times a minute.
