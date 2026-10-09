// selection-gesture.js
// Tandem's own script (not from iTerm2). Runs in Tandem's isolated world in every frame. It tells
// Tandem that the PERSON just pressed or released the pointer or a key: only trusted events count (a
// page script can dispatch fake ones, and can change the selection programmatically), at most ten
// presses and ten releases a second, and a held key's repeats are not presses. Tandem copies a page
// selection to the clipboard only when a press began it (the release copies the latest change) or a
// release came a moment before it (a double click).

(function() {
    'use strict';
    try {
        const windows = { down: { start: 0, count: 0 }, up: { start: 0, count: 0 } };
        function send(phase) {
            const w = windows[phase];
            const now = Date.now();
            if (now - w.start >= 1000) {
                w.start = now;
                w.count = 0;
            }
            if (w.count >= 10) {
                return;
            }
            w.count += 1;
            window.webkit.messageHandlers.TandemSelectionGesture.postMessage({ t: "gesture", phase: phase, sessionSecret: "{{SECRET}}" });
        }
        function on(type, phase) {
            window.addEventListener(type, function (e) { if (!e.isTrusted) { return; } if (e.repeat) { return; } send(phase); }, true);
        }
        on("pointerdown", "down");
        on("mousedown", "down");
        on("keydown", "down");
        on("pointerup", "up");
        on("mouseup", "up");
        on("keyup", "up");
    } catch (err) {
        console.error('[selection-gesture error]', 'message:', err.message);
    }
    return true;
})();
