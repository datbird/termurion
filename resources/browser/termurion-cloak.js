// termurion-cloak.js
// Tandem's own script (not from iTerm2). It runs first in the page world, as iTerm2's
// cloak-page-world.js does. In frames that belong to known captcha and fingerprinting endpoints
// (Cloudflare Turnstile, hCaptcha, Arkose/FunCaptcha, reCAPTCHA) it removes window.chrome.webview,
// so the page-world bridges that run after it capture nothing and post nothing there, as iTerm2's
// page-world bridges bail out in those frames. It changes nothing a person sees: only the channel a
// page-world script would post through. Tandem's isolated-world scripts keep their own channel and
// are unaffected. No marker is left on window.

(function() {
    'use strict';
    try {
        if (!({{INCLUDE:challenge-frame-detection.js}})) {
            return;
        }
        // No stand-in is installed if the delete fails: a stub would be a stronger fingerprint.
        try {
            delete window.chrome.webview;
        } catch (e) {}
    } catch (e) {}
})();
