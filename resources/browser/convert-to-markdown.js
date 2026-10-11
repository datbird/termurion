(function(skipChrome) {
    // Begin turndown
    {{INCLUDE:turndown.browser.umd.js}}
    // End turndown
    const service = new TurndownService({
        headingStyle: 'atx',
        codeBlockStyle: 'fenced'
    });

    service.addRule('removeMedia', {
        filter: [
            'img',
            'video',
            'audio',
            'picture',
            'source',
            'track',
            'embed',
            'object'
        ],
        replacement: () => ''
    });

    if (skipChrome) {
        service.addRule('skipChrome', {
            filter: node => ['NAV', 'FOOTER', 'ASIDE', 'HEADER'].includes(node.tagName),
            replacement: () => ''
        });
    }

    const clone = document.documentElement.cloneNode(true);
    clone
        .querySelectorAll('script, style')
        .forEach(e => e.remove());
    // Tandem's own page UI is not the page's content (mark annotations carry the person's mark
    // names): never in what the model reads. The injecting scripts are upstream's, so this list
    // names what they create.
    clone
        .querySelectorAll('#iterm-mark-annotations, .iterm-mark-annotation, .find-nav-bubble, #find-nav-styles, #iterm2-autofill-highlight-style, #iterm-copy-mode-cursor, button[aria-label="Open Password Manager"], button[aria-label^="Autofill "], [data-tandem-ui]')
        .forEach(e => e.remove());
    // Never the value of a secret field (the AI browser tools hand this markdown to a model).
    clone
        .querySelectorAll('input[type="password" i], input[type="hidden" i], [hidden], [autocomplete*="password" i], [autocomplete^="cc-" i], [autocomplete="one-time-code" i]')
        .forEach(e => e.remove());

    const bodyEl = clone.querySelector('body');

    let root;

    if (skipChrome) {
        root = clone.querySelector('main, article, [role="main"]')
            || bodyEl || clone;
    } else {
        root = bodyEl || clone;
    }
    const markdown = service.turndown(root.innerHTML);
    return markdown;
})({{SKIP_CHROME}});
