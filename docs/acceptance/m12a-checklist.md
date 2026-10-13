# Milestone 12a acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Termurion window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\termurion\target\release\` together, then run `tandem.exe`.

Setup: nothing in 12a sends a request, so no API key needs to be real; use a made-up key such as
`sk-test-1`. Have Control Panel > Credential Manager at hand, and a copy of an iTerm2 settings plist
(`com.googlecode.iterm2.plist`) that holds AI keys: `AIVendor` set to 1 (Anthropic),
`UseRecommendedAIModel` false, one entry in `AIManualModelConfigurations`, and `EnableAI` and
`AICompletions` both set to true by hand. "Settings > General > AI" means Settings, the General
tab, the AI section. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Settings > General > AI shows three sub-tabs: General, Prompts, Features. General shows Requests:, Consent:, API Key:, AI Model:, Timeout: and the checkboxes below them. | |
| 2 | In Settings > Advanced > General, turn off "Allow the use of large language model APIs?". Back in General > AI > General, everything is disabled except the Timeout field and the checkbox iTerm2 leaves on ("Enable “zero data retention” (send whole conversation every time)"). "Enable generative AI features", Set API Key…, the AI Model popup, Manage AI Models…, and "Use AI to check safety of commands before running them" are disabled. | |
| 3 | With that switch still off, open the Composer and press its AI button, then Win+Y: each shows "Feature Unavailable" / "Generative AI features have been disabled. Check with your system administrator." with no suppress checkbox. The toolbelt's Codecierge shows the administrator text and no usable button. Turn the switch back on. | |
| 4 | With "Enable generative AI features" off, press Win+Y in the Composer: "Feature Unavailable" / "You must enable AI features in settings before you can use this feature." with Reveal and Cancel. Reveal opens Settings at General > AI > General. | |
| 5 | Codecierge (Enable still off) shows its button "Reveal Setting"; clicking it opens Settings at the same place, General > AI > General. | |
| 6 | Turn "Enable generative AI features" on, with the AI Model popup at OpenAI. Requests: reads "Requests go to api.openai.com (OpenAI). Termurion asks before the first one." Press Win+Y in the Composer: "Send AI Requests to OpenAI?" names api.openai.com, with Allow and Cancel. | |
| 7 | Cancel, then Win+Y again: it asks again. Allow: nothing else happens (the action does not run by itself). Win+Y again: the Composer answers "AI features come in milestone 12." and Requests: now reads "... You allowed them." | |
| 8 | Choose Anthropic in the AI Model popup, then Win+Y: it asks again, "Send AI Requests to Anthropic?" naming api.anthropic.com. | |
| 9 | Click "Forget Allowed Hosts" (then disabled). Switch back to OpenAI and press Win+Y: it asks again for api.openai.com. | |
| 10 | Click Set API Key… ("Manage AI API Keys", "Keys are stored securely in the Windows Credential Manager."), type `sk-test-1` in the OpenAI field (placeholder "OpenAI API key") and OK. Control Panel > Credential Manager > Windows Credentials shows `TandemAI/OpenAI`; Termurion's Password Manager (Win+Alt+F) does not list it. | |
| 11 | The hint under the AI Model popup reads "Authorizes with your OpenAI API key." (before the key was set it read "No OpenAI API key is set yet."). Reopen Set API Key…: the OpenAI field shows bullets, never the key. | |
| 12 | In Set API Key…, clear the OpenAI field and OK: `TandemAI/OpenAI` is gone from Credential Manager and the hint reads "No OpenAI API key is set yet." Opening the dialog and pressing OK without touching a field changes nothing in Credential Manager. | |
| 13 | Manage AI Models…: Add, choose the Preset "Ollama (native)", enter the model "llama3", and Add. The editor's key hint reads "No API key is sent to a local endpoint. If it requires authentication, use a custom header." The sheet lists llama3 with API and Endpoint (localhost). | |
| 14 | Select llama3 and Toggle Default: its row gets the star, and the AI Model popup shows `Manual: llama3 \u{2014} Ollama` (an em dash between the two). The hint under the popup reads the self-hosted text. Win+Y asks consent for localhost. | |
| 15 | Add a second model (Custom preset, any name and an https URL) and click Economy on it: its row shows "Economy model"; hovering the Economy button shows iTerm2's tooltip about a cheaper model for background jobs. | |
| 16 | Duplicate a row: a copy named "<name> copy" appears. Edit it, add a custom header named `Bad Header` (with a space), Save: "Invalid Manual AI Model" with "Custom header name “Bad Header” is not valid. Use only RFC 7230 token characters (letters, digits, and any of ...)." (iTerm2's text, listing the allowed symbols). The model is not saved. | |
| 17 | Delete the default (starred) model: the next row becomes the default; delete every row and the AI Model popup falls back to the stored vendor's item. | |
| 18 | Tab to Duplicate, Edit, or Delete in the sheet and press Return: the button acts once and the sheet stays open (Return never also closes it). Tab to Edit and press Return: the editor opens and stays open (the same Return does not also save it); the same holds for Set API Key… and its dialog. Hold Return in the editor or the key dialog: it acts once, not again when the key repeats. | |
| 19 | Prompts: choose "AI Prompt", remove `\(ai.prompt)` from the text: a warning appears, "The prompt must contain the substring \(ai.prompt). The query you enter will replace it when speaking to the AI. ..." Put it back: the warning goes. | |
| 20 | Prompts: edit any prompt, then Reset: iTerm2's default text returns. "AI Prompt for AI Chat Orchestration" names Termurion, not iTerm2. The "?" button shows the prompt help in a scrollable window that closes with OK, Return, or Escape. The selector has eight groups separated by lines, as iTerm2's. | |
| 21 | Prompts: type a few characters into "AI Prompt", switch the selector to "AI Prompt for Chat Icon", and press Ctrl+Z there: the Chat Icon text is unchanged (Undo never brings another prompt's text across); switch back to "AI Prompt" and Ctrl+Z undoes the typing there. | |
| 22 | Features: under "Chat Permissons:" the eight popups (Check Terminal State, Run Commands, View Contents, Write to Clipboard, Control Terminal, View Manpages, Write to Filesystem, Act in Web Browser), each Ask Each Time, Allow Without Asking, or Never Allow; under "AI Completion:" the checkbox "Offer AI command suggestion in Composer & Auto Composer". Changing one survives a restart of Termurion. | |
| 23 | General: "Automatically update available AI models from live online database" and "Update Now" are shown disabled; hovering says "Termurion ships its AI model list with each release; online updates are not available." | |
| 24 | Turn Enable off. Settings > Profiles > Import from iTerm2... with the plist from Setup: the AI Model popup shows the imported vendor (Anthropic) and Manage AI Models… lists the imported manual model, but "Enable generative AI features" stays off, "Offer AI command suggestion" stays off, and no host is allowed. | |
| 25 | Add a key binding with the Toggle Setting action (Settings > Keys): its setting chooser does not offer "Enable generative AI features", "Offer AI command suggestion", the administrator switch, or the eight permissions. | |
| 26 | With the Python API enabled, a script's `SetPreference("EnableAI", true)` (and the same for `NoSyncAIRequestConsent`, `GenerativeAIAllowed`, `AICompletions` or any `AIPermission...` key) answers INVALID_VALUE, and "Enable generative AI features" stays off. GetPreference still reads them. | |

## Kept quirks and rulings a person can see

| # | Item | Result |
|---|---|---|
| 27 | iTerm2's plugin install step is replaced by a one-time consent per host (localhost included), asked after the Enable check; after Allow the person repeats the action. | |
| 28 | The gate's warnings never offer a suppress checkbox, as iTerm2's persistent warnings. | |
| 29 | A default model with no endpoint Termurion can reach (an imported Apple Intelligence vendor, a manual model without a URL) refuses with "The selected AI model has no endpoint Termurion can reach. Choose another model in Settings > General > AI." and Reveal. | |
| 30 | After the gate passes, the Composer and Codecierge still answer "AI features come in milestone 12." (the features come in 12e). | |
| 31 | Apple Intelligence is never offered in the AI Model popup, the key dialog, or the presets; the AI tab title settings are absent (n/a on Windows). | |
| 32 | Ollama's Regular and Budget pickers, Fetch Models, Refresh Models, Test Connection and the "Ollama (auto-discover)" preset's Save are disabled with "Comes in milestone 12." (12b). Choosing Ollama in the AI Model popup stores the vendor without probing. | |
| 33 | A failed key write (for example Credential Manager refusing it) shows "Could not save the <provider> API key: <error>" in the settings window and a warning mark beside Set API Key… (iTerm2 only logs it). | |
| 34 | The sheet's toolbar uses Segoe Fluent Icons glyphs, and the economy toggle and row mark are the text "Economy" / "Economy model" (iTerm2 draws a leaf). | |
| 35 | The editor's final button reads "Add" while adding and "Save" while editing, as iTerm2's source. An empty model name shows blank in the sheet and duplicates to " copy". | |
| 36 | "Chat Permissons:" keeps iTerm2's typo. | |
| 37 | The prompt help shows plain text (no rendered links). | |
| 38 | Timeout accepts 0 and any positive number of seconds, with no range, as iTerm2's. | |
| 39 | Advanced AI rows that hold text or numbers (`aiProxy`, `ollamaNumCtx`, `ollamaKeepAlive`, `unameCommand`, `llmPlatform`, `aiModernModelPrefixes`, `aiModelCatalogURL`) are stored but not editable in Settings until milestone 13; `GenerativeAIAllowed` sits under Advanced > General, as iTerm2's. | |
