// selection-gesture.js
// Tandem's own script (not from iTerm2). Runs in Tandem's isolated world in every frame. It tells
// Tandem that the PERSON just used the pointer or the keyboard: only trusted events count (a page
// script can dispatch fake ones, and can change the selection programmatically), at most ten a
// second. Tandem copies a page selection to the clipboard only when one of these arrived a moment
// before the selection changed.

(function() {
    'use strict';
    try {
        let windowStart = 0;
        let count = 0;
        function send() {
            const now = Date.now();
            if (now - windowStart >= 1000) {
                windowStart = now;
                count = 0;
            }
            if (count >= 10) {
                return;
            }
            count += 1;
            window.webkit.messageHandlers.TandemSelectionGesture.postMessage({ t: "gesture", sessionSecret: "{{SECRET}}" });
        }
        function on(type) {
            window.addEventListener(type, function (e) { if (!e.isTrusted) { return; } send(); }, true);
        }
        on("pointerup");
        on("mouseup");
        on("keyup");
    } catch (err) {
        console.error('[selection-gesture error]', 'message:', err.message);
    }
    return true;
})();
