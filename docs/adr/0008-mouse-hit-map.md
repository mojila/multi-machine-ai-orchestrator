# ADR-0008: Pointer input via a render-time hit map, with strict keyboard parity

- **Status:** Proposed
- **Date:** 2026-10-21
- **Deciders:** project owner
- **Related:** [PLAN.md §2.3 TUI specification](../PLAN.md#23-tui-specification),
  [PLAN.md §3.8 TUI architecture](../PLAN.md#38-tui-architecture),
  [ADR-0003 engine owns store](0003-engine-owns-store.md)

## Context

The TUI must support mouse click and scroll: selecting cards and columns, opening a card, wheeling
through long columns and transcripts, dragging scrollbars, clicking palette rows, and clicking
approval buttons. ratatui/crossterm give us `MouseEventKind` events but **no hit testing**: nothing in
the framework knows which widget occupies a screen cell, because widgets are drawn through a
`Buffer` with no retained geometry.

Two things make this more than a convenience feature:

1. **Terminal capture is a global, sticky mode.** Enabling mouse capture (`EnableMouseCapture`) makes
   the terminal report pointer events instead of performing native drag-select. If the process dies
   without disabling capture, the user's shell is left unable to select text until they run `reset`.
   The existing RAII guard already handles this hazard for bracketed paste and alternate screen;
   mouse must join it.
2. **Pointer input is a second input path into privileged actions.** A click on "approve" must not be
   able to do anything a keypress could not: the approval, confirmation, and policy machinery must be
   reached identically.

Options considered:

1. **Ad-hoc coordinate branching** in the event handler (`if row == 3 && col > 20 { ... }`).
2. **A widget tree with retained state** (immediate-mode with persistent widget IDs).
3. **A hit map produced by the pure render function**, mapping screen rectangles to semantic regions,
   resolved through the same action table the keymap uses.

Option 1 duplicates layout knowledge and rots the moment a column width or breakpoint changes. Option 2
imports retained-mode complexity into an otherwise Elm-ish design and makes snapshot-only testing
harder. Option 3 keeps rendering pure and makes hit testing a data structure that unit tests can
assert directly.

## Decision

1. **`view(&AppState, &mut Frame) -> HitMap`.** The pure render returns a `HitMap`: an ordered list of
   `(Rect, RegionId)` in draw order plus a layout version. The render loop stores the latest map in
   `AppState`. Later regions win, which gives overlays precedence over the board for free.
2. **One action vocabulary.** `RegionId` resolves to an `Action` through the same table the keymap uses.
   There is no pointer-only action, no pointer-only overlay, and no pointer-only code path in
   `update()`. This is the invariant that keeps the security model intact: a click can never approve a
   destructive action without the confirmation a keypress would trigger.
3. **Capture is symmetric with paste.** Mouse capture is enabled and disabled by the same RAII guard
   as bracketed paste, so it is released on normal exit, on panic (panic hook, installed before
   `ratatui::init`), and on `SIGTERM`. `mouse = false` is a supported pure-keyboard session and is how
   a user keeps native drag-select.
4. **`Shift`+drag passes through** to terminal-native selection, which is the conventional escape
   hatch; `y` / OSC-52 copy remains the in-app alternative.
5. **Motion is opt-in.** `Moved` events are ignored unless `[ui].hover = true`, and hover may only
   recolour a region — never change selection, scroll offset, or focus. Terminals emit motion at
   pointer rate, so treating it as free would create needless redraw churn.
6. **Stale input is dropped.** A click is applied only if its coordinates still fall inside the
   recorded rect and the referenced entity still exists; after a resize or board change the event is
   discarded rather than guessed against a new layout.
7. **Wheel and drag are bounded.** `[ui].scroll_lines` (default 3) sets the wheel-notch distance;
   scroll positions clamp at both ends (no overscroll, no wrapping); scrollbar drag maps pointer
   position to offset arithmetically and shares the same clamping.
8. **Outside-click means `Esc`,** except on confirm dialogs, which ignore outside clicks — the same
   as they ignore `Esc`.

## Consequences

**Positive:** the board, run panel, palette, and approval dialogs gain a natural interaction model
without touching `update()`; because regions are semantic, "click equals key" is a property test over
the whole keymap rather than a hand-maintained list; the hit map is returned data, so it is asserted in
unit tests with no terminal, and screenshot snapshots stay valid because mouse state does not affect
drawing. Keyboard parity means `mouse = false` loses no functionality, which also keeps the TUI usable
over ssh and in terminals with poor mouse reporting.

**Negative:** every new widget or overlay must register its regions, and forgetting to do so is a
silent "this bit isn't clickable" defect rather than a compile error; hover, double-click, and drag
add timing state that must be reset on layout change; the hit map is one more allocation per frame
(small — the region count is bounded by visible cards and rows, and it is built into a reused
`Vec`). Capturing the mouse changes terminal selection behaviour, which will surprise users once and
must be documented in the README and the help overlay.

**Follow-ups:** add a debug overlay (`:debug hitmap`) that draws region outlines and IDs to make
missing registrations obvious; consider keyboard-driven scrollbar focus for parity of every drag
scenario; revisit whether drag-to-reorder cards between columns should be a pointer gesture or stays
keyboard-only (`H`/`L`) for v0.1 — currently keyboard-only, so pointer semantics stay simple.
