// Applies a saved explicit theme (light or dark) before first paint. Loaded synchronously from <head> (CSP script-src
// 'self', no inline script, no hash). The key and the values must match src/lib/theme/theme.ts. With no saved choice,
// nothing is set and app.css follows prefers-color-scheme. Storage access can throw (blocked site data), so it is wrapped.
(function () {
  try {
    var choice = window.localStorage.getItem('theme');
    if (choice === 'light' || choice === 'dark') document.documentElement.dataset.theme = choice;
  } catch {
    // storage refused: System
  }
})();
