// broadcast-input.js
// Tandem's own script (not from iTerm2). It runs in Tandem's isolated world, in the main frame, at
// document start. It reports what the person types so Tandem can type the same into the other browser
// panes of the broadcast set (iTerm2 sends `insertText:` and `doCommandBySelector:` from the web view,
// `iTermBrowserWebView.swift:1033-1047`): the text of each trusted beforeinput, and the editing keys.
// Tandem decides whether anything is replayed; this script only reports. A synthetic event (isTrusted
// false) reports nothing, and a field that holds a secret (a password, a one-time code, a card
// number) reports nothing, neither text nor keys.

(function() {
    'use strict';
    try {
        const SECRET = "{{SECRET}}";
        const SENSITIVE = {{SELECTOR}};
        const MAX_TEXT = 1048576;
        // The editing keys that are sent, with their virtual key codes.
        const KEYS = {
            Backspace: 8, Tab: 9, Enter: 13, Escape: 27, PageUp: 33, PageDown: 34, End: 35, Home: 36,
            ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40, Delete: 46
        };
        // At most 200 posts a second.
        const win = { start: 0, count: 0 };
        function post(body) {
            const now = Date.now();
            if (now - win.start >= 1000) {
                win.start = now;
                win.count = 0;
            }
            if (win.count >= 200) {
                return;
            }
            win.count += 1;
            body.sessionSecret = SECRET;
            window.webkit.messageHandlers.tandemBroadcastInput.postMessage(body);
        }
        // True when the event's target (through shadow roots) or the focused field holds a secret; any
        // failure counts as true.
        function secret(e) {
            try {
                const t = e.composedPath ? e.composedPath()[0] : e.target;
                if (t && t.nodeType === 1 && t.matches(SENSITIVE)) {
                    return true;
                }
                let a = document.activeElement;
                for (let i = 0; i < 32 && a; i++) {
                    if (a.matches && a.matches(SENSITIVE)) {
                        return true;
                    }
                    a = a.shadowRoot ? a.shadowRoot.activeElement : null;
                }
            } catch (x) {
                return true;
            }
            return false;
        }
        window.addEventListener('beforeinput', function(e) {
            if (!e.isTrusted) { return; }
            if (e.isComposing || secret(e)) { return; }
            let text = null;
            if (e.inputType === 'insertText') {
                text = e.data;
            } else if (e.inputType === 'insertLineBreak') {
                text = '\n';
            } else if (e.inputType === 'insertFromPaste') {
                text = e.data || (e.dataTransfer ? e.dataTransfer.getData('text/plain') : null);
            }
            if (typeof text === 'string' && text.length > 0 && text.length <= MAX_TEXT) {
                post({ t: 'insert', text: text });
            }
        }, true);
        window.addEventListener('keydown', function(e) {
            if (!e.isTrusted) { return; }
            if (e.isComposing || e.ctrlKey || e.altKey || e.metaKey || secret(e)) { return; }
            const code = KEYS[e.key];
            if (code === undefined || typeof e.code !== 'string' || !/^[A-Za-z0-9]{1,32}$/.test(e.code)) { return; }
            post({ t: 'key', key: e.key, code: e.code, keyCode: code, mods: e.shiftKey ? 1 : 0 });
        }, true);
    } catch (err) {
    }
})();
