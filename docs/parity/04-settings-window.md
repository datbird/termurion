# iTerm2 Settings window surfaces

One row per control or editor of iTerm2's Settings window that had no row in 01 to 03 (the UI parity sweep, package S, `superpowers/plans/2026-10-14-ui-parity-sweep.md`). The labels of every Settings row are held to iTerm2's xib wording by `src/ui_labels.rs` (generated table `schema/ui_labels.toml`, `scripts/gen-ui-labels.py`). These rows are kept out of 01 to 03 because the roadmap states those files' counts; the merge moves them there and updates the counts.

| Surface | Control | What it does | Termurion status | Windows note |
|---|---|---|---|---|
| Settings window | Search field and results list | Typing in the toolbar field lists the settings whose name holds every word (prefix or stem); Up, Down and Enter, or a click, open its pane and tab and mark the control. | have | `settings_search` (`field`, `search`, `flash_if_marked`). Documents are the schema rows' labels, the Advanced pane's descriptions and no other text. |
| Settings window | Search Options menu | "Show indicators for non-default values". | have | `settings_ui::SHOW_INDICATORS`; the magnifier at the right of the page bar. |
| Settings window | Field "Reset to ..." menu | A text or number field whose value is not the default starts its context menu with "Reset to <default>" ("Reset to Empty Default" for an empty one). | have | `settings_keys::reset_menu` and `reset_value`, on every profile and global text and number field. Cut, Copy and Paste are not in the menu (egui has no text menu). |
| Profiles list | Search field menu | A magnifier menu: the heading "Tags", every tag (adds " tag:<tag>" to the query), then "Search Syntax Help". | have | `profile_list_ui::profile_list`; the help opens Termurion's `search-syntax` page, not iterm2.com. |
| Profiles list | Other Actions... menu | Set as Default, Duplicate Profile, Bulk Copy from Selected Profile..., Save Profile as JSON..., Save All Profiles as JSON..., Import JSON Profiles.... | have | `settings_profile_actions`; "Import from iTerm2..." stays below them as Termurion's own item. |
| Profiles list | Bulk Copy sheet | "Copy these settings from profile "<name>":" with Colors, Text, Window, Terminal, Keys, Session and Advanced, "To these profiles:" (a multi-select profile list), Copy and Cancel. | have | `settings_profile_actions::show` and `bulk_copy`. No Web box: Termurion's Browser rows are on General. A hotkey is never copied. |
| Profiles > General | Tags | Token field. | have | `settings_lists::token_field`. |
| Profiles > General | Title | Popup of check items and script title functions. | have | `settings_title`. |
| Profiles > General | Subtitle and Enable Tall Tab Bar | The subtitle needs a tab bar 28 points tall or the Minimal theme; the button sets the height. | have | `settings_ui::subtitles_allowed`, `ENABLE_TALL_TAB_BAR`. |
| Profiles > Text | Font and Non-ASCII Font | Family, member (weight) and size, spacings, Anti-aliased with its hint, ligatures with its hint. | have | `settings_fonts`; weight by `Renderer::resolve_weight`. |
| Profiles > Text | Font features button | Stylistic sets and contextual alternates per font. | missing | Not on the sweep's list. iTerm2 keeps CoreText feature settings in the font descriptor; Termurion's font string is a name and a size, and Contextual Alternates is the global Advanced setting `EnableContextualAlternates`. Owner: the sweep rulings (S-8). |
| Profiles > Window | Screen | Popup of No Preference, Screen with Cursor and the monitors. | have | `settings_ui::screen_row`. |
| Profiles > Session | Note under Buffer Input by Default | "You may want a trigger to stop buffering". | have | `settings_ui::BUFFERING_NOTE`. |
| Profiles > Advanced | Snippets Filter | Token field. | have | `settings_lists::tokens_row`. |
| Smart Selection editor | Rule table, +, -, Load Defaults | The rules as name, /regex/ and precision with the action count; Load Defaults puts the built-in list back. | have | `settings_advanced::smart_selection_rules`. |
| Smart Selection editor | Name, Regular expression, Precision, Actions... | The selected rule's fields; Precision is Very Low to Very High. | have | Same function; Actions... opens the actions block under the fields. |
| Smart Selection editor | Syntax Help | Opens help on the expressions. | have | Opens Termurion's `smart-selection` page (the dialect is Rust's, not ICU). |
| Smart Selection editor | Open Regular Expression Visualizer | Shows the expression as nested boxes. | have | `regex_outline`; a read of the expression, not iTerm2's SVG railroad diagram. |
| Smart Selection editor | Playground | Click in sample text to see which rule wins there. | have | `settings_advanced::playground_result`. |
| Smart Selection editor | Log debug info to Console.app | Logs each smart selection decision. | n/a | Console.app is macOS only. |
| Pointer | Load Defaults | Puts the factory pointer bindings back. | have | `settings_keys::LOAD_DEFAULTS`. |
| General | Window restoration policy | One popup over the two startup switches. | have | `settings_keys::restoration_row`. |
| General | Allow sending of clipboard contents? | Ask Each Time, Never or Always. | have | `settings_keys::clipboard_sending_row`. |
| General | Memory usage per session, tmux pause limit | Integer fields. | have | `settings_keys::global_control`. |
| Appearance | New tabs open at end of tab bar | Checkbox; off puts a new tab next to the current one. | have | `AddNewTabAtEndOfTabs`, `App::add_tab`. |
| General | GPU rendering | A master switch for the GPU renderer. | n/a | Termurion has no renderer but the GPU one. |
