// Prevent Prism from scanning or mutating the DOM on load; highlighting runs as pure tokenization.
;(globalThis as { Prism?: { manual?: boolean } }).Prism = { manual: true }
