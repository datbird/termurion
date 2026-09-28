# Milestone 12b acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Setup: AI enabled in Settings as 12a leaves it ("Enable generative AI features" on). A real API key
for at least one cloud vendor (OpenAI, Anthropic, Gemini or DeepSeek), stored with Set API Key…; mark
a vendor's rows "not applicable" when no key for it is at hand. Ollama installed and running with at
least one model pulled (for example `ollama pull llama3.2`) for the Ollama rows. Fiddler Classic
listening on 127.0.0.1:8888 with "Decrypt HTTPS traffic" off (a proxy that decrypts HTTPS fails every
row through it, because WinHTTP rejects its certificate). The test server below, saved as `ai-test-server.ps1`, stands in for a local
OpenAI-compatible server (stop LM Studio first if it uses port 1234); run it with
`powershell -ExecutionPolicy Bypass -File ai-test-server.ps1 -Port 1234 -Mode ok` and close its
window to stop it. Modes: `ok` answers at once, `auth` answers 401 unless the request carries
`Authorization: Bearer sk-local-test`, `stall` waits 20 seconds before it answers (longer than row
14's 5 second timeout; the server takes the next request only after that, so wait 20 seconds
between two presses, or close and restart it), `slow` sends the answer in ten pieces two seconds
apart, `huge` answers with 65 MiB.

```
param([int]$Port = 1234, [string]$Mode = 'ok')
$answer = '{"choices":[{"index":0,"message":{"role":"assistant","content":"Hello"},"finish_reason":"stop"}]}'
$listener = New-Object System.Net.Sockets.TcpListener -ArgumentList ([System.Net.IPAddress]::Loopback), $Port
$listener.Start()
Write-Host "Test server on http://localhost:$Port/ in mode $Mode; close this window to stop it."
while ($true) {
  $client = $listener.AcceptTcpClient()
  try {
    $stream = $client.GetStream()
    $buffer = New-Object byte[] 65536
    $request = ''
    while ($true) {
      $n = $stream.Read($buffer, 0, $buffer.Length)
      if ($n -le 0) { break }
      $request += [System.Text.Encoding]::ASCII.GetString($buffer, 0, $n)
      $end = $request.IndexOf("`r`n`r`n")
      if ($end -ge 0) {
        $length = 0
        if ($request -match '(?im)^content-length:\s*(\d+)') { $length = [int]$Matches[1] }
        if ($request.Length -ge $end + 4 + $length) { break }
      }
    }
    $body = $answer
    $status = '200 OK'
    if ($Mode -eq 'auth' -and $request -notmatch '(?im)^authorization:\s*Bearer sk-local-test\s*$') {
      $body = '{"error":{"message":"Unauthorized"}}'
      $status = '401 Unauthorized'
    }
    if ($Mode -eq 'huge') { $body = 'x' * 68157440 }
    if ($Mode -eq 'stall') { Start-Sleep -Seconds 20 }
    $head = [System.Text.Encoding]::ASCII.GetBytes("HTTP/1.1 $status`r`nContent-Type: application/json`r`nContent-Length: $($body.Length)`r`nConnection: close`r`n`r`n")
    $stream.Write($head, 0, $head.Length)
    $stream.Flush()
    $bytes = [System.Text.Encoding]::ASCII.GetBytes($body)
    $piece = $bytes.Length
    if ($Mode -eq 'slow') { $piece = [Math]::Ceiling($bytes.Length / 10) }
    for ($i = 0; $i -lt $bytes.Length; $i += $piece) {
      if ($Mode -eq 'slow') { Start-Sleep -Seconds 2 }
      $stream.Write($bytes, $i, [Math]::Min($piece, $bytes.Length - $i))
      $stream.Flush()
    }
  } catch {
  } finally {
    $client.Close()
  }
}
```

`aiProxy` has no field in Settings until milestone 13: set it by closing Tandem and adding
`aiProxy = "127.0.0.1:8888"` under `[settings]` in `%APPDATA%\Tandem\settings.toml`, then starting
Tandem again; remove the line the same way. "The editor" means Settings > General > AI > General,
Manage AI Models…, then Add (or Edit on a row). Test Connection asks consent for a host once, the
first time: answer Allow and press Test Connection again (it does not run by itself). Mark each
line pass or fail.

## Test Connection

| # | Check | Result |
|---|---|---|
| 1 | In the editor choose the Preset `gpt-5-mini` (OpenAI Responses at `https://api.openai.com/v1/responses`) and press Test Connection. The first time: "Send AI Requests to OpenAI?" naming api.openai.com; Allow, then Test Connection again. The button reads "Testing…" with a spinner and is disabled, then "Connection Succeeded" with "The connection succeeded. The model responded normally." | |
| 2 | With the consent not yet given for a host (for example after Forget Allowed Hosts), press Test Connection twice before answering the consent alert: the alert shows once, not twice in a row, and the second press brings it to the front if another window covered it. | |
| 3 | Preset Custom, URL `https://api.openai.com/v1/chat/completions`, API "Chat Completions", model `gpt-4o-mini`: Test Connection succeeds. | |
| 4 | Preset `claude-haiku-4-5` (Anthropic): Test Connection succeeds after the consent for api.anthropic.com. | |
| 5 | Preset `gemini-2.5-flash` (Google Gemini, URL `https://generativelanguage.googleapis.com/v1beta/models/{{MODEL}}`): Test Connection succeeds after the consent for generativelanguage.googleapis.com. | |
| 6 | Preset `deepseek-v4-flash` (DeepSeek): Test Connection succeeds after the consent for api.deepseek.com. | |
| 7 | Set API Key…: replace one vendor's key with a wrong one (for example `sk-wrong-0000000000000000`) and OK. Test Connection on that vendor's model: "Connection Failed" with the provider's own reason (OpenAI "Incorrect API key provided: ...", Anthropic "invalid x-api-key", Gemini "API key not valid. Please pass a valid API key."), and no withheld-key advice (a cloud host). For Gemini the text never shows the key. Put the real key back. | |
| 8 | Set API Key…: clear one vendor's key and OK. Test Connection on that vendor's model opens the key prompt ("Manage AI API Keys", one masked field named for the vendor, OK and Cancel). Cancel: nothing is shown, and Test Connection is enabled again. Test again, type the real key, OK: the test runs and succeeds, and Credential Manager shows `TandemAI/<vendor>` again. | |
| 9 | With the test server in mode `ok` on port 1234: Preset "Local OpenAI-compatible (LM Studio, vLLM, …)", URL `http://localhost:1234/v1/chat/completions`, any model name. Test Connection asks consent for localhost, then succeeds, and no key prompt appears (a local server never gets the vendor key). | |
| 10 | Restart the test server in mode `auth`. Test Connection on the same entry: "Connection Failed" with "Unauthorized", a blank line, then "Tandem did not send an API key with this request. If this endpoint requires authentication, add the header “Authorization” under “Custom headers”." | |
| 11 | Add the custom header `Authorization` with the value `Bearer sk-local-test` and Test Connection again: "Connection Succeeded". | |
| 12 | Clear the model name and press Test Connection: "Missing Information" with "Enter a model name and URL before testing the connection." Nothing is sent. | |
| 13 | Enter a model name and the URL `not a url`: Test Connection shows "Connection Failed" with "The URL is not valid." at once, with no consent alert and no key prompt. | |
| 14 | Settings > General > AI > General, Timeout: 5. Test server in mode `stall` on port 1235, an entry at `http://localhost:1235/v1/chat/completions`: after about 5 seconds, "Connection Failed" with "HTTP request failed with The request timed out." | |
| 15 | Test server in mode `slow` on port 1235, Timeout still 5: Test Connection runs for about 20 seconds and succeeds (the timeout counts idle time between bytes, not the whole request). Set Timeout back to its old value. | |
| 16 | Set API Key…: type `placeholder` as the OpenAI key and OK. With `aiProxy` set to Fiddler (Setup) and Fiddler's capture off, Test Connection on the `gpt-5-mini` entry: "Connection Succeeded" with "Placeholder key in use: AI responses are simulated for App Review and no external service is contacted.", and Fiddler shows no new session to api.openai.com. Put the real key back. | |

## Ollama

| # | Check | Result |
|---|---|---|
| 17 | Settings > General > AI > General: choose Ollama in the AI Model popup. A sheet "Contacting Ollama…" with "Fetching the models installed on your local Ollama server.", a spinner and Cancel shows briefly; then Regular: and Budget: pickers appear under the popup. Regular lists the installed models; Budget starts with "Same as regular model". No consent is asked for this (discovery sends no user data). | |
| 18 | Pick a model in Regular and one in Budget; close and reopen Settings: both picks are kept. | |
| 19 | Choose OpenAI in the popup, stop Ollama (quit it from the tray), then choose Ollama again: after the probe, "Could Not Reach Ollama" with "Could not reach the Ollama server at http://localhost:11434/api/chat. Make sure Ollama is running, then try again.", and the popup shows OpenAI again. Start Ollama again. | |
| 20 | In the editor, Preset "Ollama (native)", then Fetch Models: a small list of the installed model names; click one and the Model field takes it. Press Escape on the list instead: it closes and nothing changes. Cancel the editor. | |
| 21 | With Ollama stopped, Fetch Models: "Could Not Fetch Models" with "Could not reach Ollama at localhost: Could not connect to the server." Start Ollama again. | |
| 22 | Preset "Ollama (auto-discover)": the button reads Refresh Models. Press it: "Models Refreshed" with "Found N installed models. Choose one from the Model popup, or “All installed models” to expose every one." (N the count). Choose "All installed models" and Save. With Ollama stopped, Refresh Models shows "Could Not Reach Server" with "Could not reach the Ollama server to list its models. Check that it is running and the URL is correct." | |
| 23 | After the Save in row 22, the AI Model popup lists one item per installed model, `Manual: <tag> (em dash) Ollama`, with an em dash where "(em dash)" stands. Saving a second auto-discover entry for the same server is refused with "There is already an auto-discover entry for this server." | |
| 24 | Start a second Ollama server on another port (`$env:OLLAMA_HOST='127.0.0.1:11435'; ollama serve` in a separate PowerShell window) and add an auto-discover entry at `http://127.0.0.1:11435/api/chat`. The AI Model popup now names each shared tag with its server, for example `Manual: llama3.2 (http://localhost:11434) (em dash) Ollama` and `Manual: llama3.2 (http://127.0.0.1:11435) (em dash) Ollama`. | |
| 25 | Test Connection on the Ollama (native) entry, and on the auto-discover entry with a model chosen in its Model popup: each asks consent for its host once, then succeeds. | |

## Proxy and logging

| # | Check | Result |
|---|---|---|
| 26 | With `aiProxy` = `127.0.0.1:8888` (Setup) and Fiddler's capture off, Test Connection on the `gpt-5-mini` entry succeeds and Fiddler shows a tunnel to api.openai.com:443. Test Connection on the Ollama (native) entry and Refresh Models add nothing to Fiddler (the proxy is for HTTPS only, and discovery never uses it). | |
| 27 | Set `aiProxy` to `127.0.0.1` (no port) or `127.0.0.1:x`: it is ignored and Test Connection goes direct (nothing in Fiddler). | |
| 28 | Remove `aiProxy` and turn Fiddler's capture on (Fiddler becomes the Windows proxy), or set a manual proxy of 127.0.0.1:8888 in Windows Settings > Network & internet > Proxy: Test Connection on the `gpt-5-mini` entry shows a tunnel to api.openai.com:443 in Fiddler (the system proxy is honored). Turn the capture or the manual proxy off again. | |
| 29 | Settings > Advanced > Debugging shows "Log raw AI API requests and responses to disk." and "Log AI chat traffic to the system console."; hovering the first shows its help, which names `%APPDATA%\Tandem\AIChatWire\` and says every header value except known non-secret ones (such as Content-Type), and keys in URLs or bodies, are replaced by fingerprints (the first 8 and last 4 characters). | |
| 30 | Turn both on and run Test Connection and Refresh Models: no `%APPDATA%\Tandem\AIChatWire\` folder appears, and DebugView (Capture Win32 on) shows no "Tandem AI:" lines. In 12b only the chat turn writes the wire log and the console entries, and the chat comes with 12c; the key fingerprints are covered by the unit tests. Turn both off. | |

## File jobs

| # | Check | Result |
|---|---|---|
| 31 | Nothing in 12b uploads a file or makes a vector store: through rows 1 to 28 with `aiProxy` on Fiddler, no request goes to api.openai.com/v1/files or /v1/vector_stores. The file jobs (create a store, upload a file, add files and wait for OpenAI to ingest them) are built and unit tested for 12c's chat, which is the first to run them. | |

## Kept quirks and rulings a person can see

| # | Item | Result |
|---|---|---|
| 32 | Test Connection asks the one-time consent for the probe's host before its request, even with the App Review key `placeholder` stored (which then contacts nothing). | |
| 33 | Ollama discovery (the probe sheet, Regular and Budget, Fetch Models, Refresh Models and the background refresh every 5 minutes) reaches the configured Ollama servers without asking consent, as iTerm2's (it sends no user data). It runs only while AI features are allowed. | |
| 34 | A localhost or private-address model never gets the vendor key: it gets `Placeholder for self-hosted` unless a custom header carries a credential, and the withheld-key advice explains it. | |
| 35 | A timeout counts idle time between bytes; a Timeout of 0 means no timeout at all. | |
| 36 | The five common connection failures read in Apple's words, as iTerm2's ("The request timed out.", "A server with the specified hostname could not be found.", "Could not connect to the server.", "The network connection was lost.", "An SSL error has occurred and a secure connection to the server cannot be made."); rarer ones read in Windows' own WinHTTP words. | |
| 37 | A redirect is followed only to the same host, and never from https down to plain http; any other redirect fails with "HTTP request failed with A redirect to another host (<host>) was not followed." or "... A redirect to plain http was not followed." (iTerm2 follows any redirect). | |
| 38 | A `user:password@` in a model or Ollama URL is not sent (WinHTTP takes no userinfo); use an `Authorization` custom header instead. The server shown for a dynamic Ollama entry leaves the userinfo out, as iTerm2. | |
| 39 | The Fetch Models names list is a small list over the editor (Escape or a click outside closes it), where iTerm2 pops a menu under the button; the probe sheet's spinner is round. | |
| 40 | Closing the Settings window while "Contacting Ollama…" shows keeps Ollama as the chosen model even if the server then turns out to be down (iTerm2's sheet cannot be left that way). | |
| 41 | File jobs, when 12c runs them, go to OpenAI only for a model whose own host is api.openai.com; any other model fails with "Files can be sent to OpenAI only with a model at api.openai.com, so that another server’s key or headers never reach OpenAI." before any request, and the App Review key `placeholder` never uploads a file (iTerm2 sends both). | |
| 42 | Test server in mode `huge` on port 1235 and the entry from row 14: Test Connection shows "Connection Failed" with "HTTP request failed with resource exceeds maximum size" (every answer is capped at 64 MiB; iTerm2 has no cap), and Tandem's memory in Task Manager does not keep growing. | |
