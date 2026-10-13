// pointer-gate.js
// Termurion's own script (not from iTerm2). It runs in Termurion's isolated world, in the main frame, at
// document start. WebView2 gives the host no mouse hook before the page, so a press a pointer
// binding claims has to be taken here, synchronously, before the page's own handlers see it
// (iTerm2 decides in the web view's mouseDown, `iTermBrowserWebView.swift:383-490`). Only the
// presses in the table Termurion keeps in `__tandemGate` are taken: right, middle and other buttons
// that are bound, and plain Ctrl single left when it stands for a right click. Left clicks stay the
// page's, except the fourth click (Smart Selection) and a single Win-click (the first action of the
// best Smart Selection rule). Every listener ignores an event whose isTrusted is false: a page that
// dispatches a synthetic press runs no binding, pastes nothing and focuses nothing.

(function() {
    'use strict';
    try {
        const SECRET = "{{SECRET}}";
        // A press this script took: its button is down (until the release), then the click that follows
        // the release is the page's no more for a second.
        const held = { 0: false, 1: false, 2: false };
        const after = { 0: 0, 1: 0, 2: 0 };
        const last = { 0: 1, 1: 1, 2: 1 };
        const win = { start: 0, count: 0 };

        function table() {
            const g = globalThis.__tandemGate;
            return g && Array.isArray(g.keys) ? g : null;
        }
        // At most 30 posts a second.
        function post(body) {
            const now = Date.now();
            if (now - win.start >= 1000) {
                win.start = now;
                win.count = 0;
            }
            if (win.count >= 30) {
                return;
            }
            win.count += 1;
            body.sessionSecret = SECRET;
            window.webkit.messageHandlers.tandemPointer.postMessage(body);
        }
        // iTerm2's button numbers: 0 left, 1 right, 2 middle, 3 and up as the page counts them.
        function buttonOf(js) {
            return js === 0 ? 0 : (js === 1 ? 2 : (js === 2 ? 1 : (js >= 3 && js <= 31 ? js : -1)));
        }
        // 1 Shift, 2 Ctrl, 4 Alt, 8 Win.
        function modsOf(e) {
            return (e.shiftKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.altKey ? 4 : 0) | (e.metaKey ? 8 : 0);
        }
        // `pointer::key`: "Button,<button>,<clicks>,<mods in the order c o s ^>,".
        function keyOf(button, clicks, mods) {
            let m = '';
            if (mods & 8) { m += 'c'; }
            if (mods & 4) { m += 'o'; }
            if (mods & 1) { m += 's'; }
            if (mods & 2) { m += '^'; }
            return 'Button,' + button + ',' + clicks + ',' + m + ',';
        }
        function swallow(e) {
            e.preventDefault();
            e.stopImmediatePropagation();
        }
        function clicksOf(e) {
            return e.detail > 0 ? e.detail : 1;
        }
        function point(e) {
            return { x: e.clientX, y: e.clientY };
        }

        function onDown(e, g) {
            const b = buttonOf(e.button);
            if (b < 0) {
                return;
            }
            const clicks = clicksOf(e);
            const mods = modsOf(e);
            last[e.button] = clicks;
            held[e.button] = false;
            after[e.button] = 0;
            // Focus on right or middle click comes first, bound or not (iTerm2's rightMouseDown and
            // otherMouseDown focus before the binding, `BWV:459-488`); an unbound press stays the page's.
            if (g.focus && b >= 1) {
                post({ type: "focus" });
            }
            // The fourth click is Smart Selection, kept even with double-click Smart Selection on.
            if (b === 0 && clicks === 4 && g.fourClick) {
                swallow(e);
                const p = point(e);
                post({ type: "smart", x: p.x, y: p.y });
                return;
            }
            if (g.keys.indexOf(keyOf(b, clicks, mods)) >= 0) {
                swallow(e);
                held[e.button] = true;
                const p = point(e);
                post({ type: "down", button: e.button, clicks: clicks, mods: mods, x: p.x, y: p.y });
                return;
            }
        }
        function onUp(e, g) {
            const b = buttonOf(e.button);
            if (b < 0) {
                return;
            }
            const mods = modsOf(e);
            if (held[e.button]) {
                swallow(e);
                held[e.button] = false;
                after[e.button] = Date.now();
                const p = point(e);
                post({ type: "up", button: e.button, clicks: last[e.button], mods: mods, x: p.x, y: p.y });
                return;
            }
            // A single click with only Win held: the best match, and its rule's first action if it has one.
            // The click goes on to the page (`BWV:531-546`).
            if (b === 0 && clicksOf(e) === 1 && mods === 8) {
                const p = point(e);
                post({ type: "smartAction", x: p.x, y: p.y });
            }
        }
        // The click, auxclick and contextmenu that follow a taken press are the page's no more.
        function onAfter(e) {
            const t = after[e.button];
            if (t !== undefined && t !== 0 && Date.now() - t < 1000) {
                swallow(e);
            }
        }

        const on = function(type, fn) {
            window.addEventListener(type, function (e) { if (!e.isTrusted) { return; } const g = table(); if (!g) { return; } fn(e, g); }, true);
        };
        on("mousedown", onDown);
        on("mouseup", onUp);
        on("auxclick", onAfter);
        on("contextmenu", onAfter);
        on("click", onAfter);
    } catch (err) {
        console.error('[pointer-gate error]', 'message:', err.message);
    }
    return true;
})();
