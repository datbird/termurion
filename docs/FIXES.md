# Fixes and tweaks (beta)

Things found while using Termurion after 0.0.1-beta1. Add new items at the end of Open. Each
item says what is wrong, what it should do, and where it was seen. Move an item to Done with its
commit. The list from before the beta is in [`TWEAKS.md`](TWEAKS.md).

## Open

(none)

## In progress

(none)

## Done

1. **Session > Move Session > Move Session to Window does nothing.** Seen 2026-10-14 with two
   windows open. Cause: the item moves the focused session into a NEW window, as iTerm2's does
   (`PseudoTerminal.m` `moveSessionToWindow:`). When the window holds only one session there is
   nothing to split off, so Termurion returns silently. iTerm2 greys the item out in that case
   (`validateMenuItem`: enabled only when the window has more than one session, and never for a
   locked session). Termurion leaves it enabled. Done 2026-10-14: both items now grey out by iTerm2's rule, per window
   (`menu::move_session_enabled`, test `move_session_items_follow_iterm2`).
