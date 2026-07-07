// Theme store: tri-state (auto | light | dark), persisted in localStorage.
// Boot script in BaseLayout applies data-theme synchronously before hydration
// to avoid a flash. This module keeps the choice in sync at runtime.

export type ThemeChoice = 'auto' | 'light' | 'dark';

const STORAGE_KEY = 'dsoc_theme';

export function readChoice(): ThemeChoice {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    if (v === 'light' || v === 'dark' || v === 'auto') return v;
  } catch {
    /* storage may be blocked */
  }
  return 'auto';
}

export function writeChoice(choice: ThemeChoice) {
  try {
    if (choice === 'auto') localStorage.removeItem(STORAGE_KEY);
    else localStorage.setItem(STORAGE_KEY, choice);
  } catch {
    /* ignore */
  }
}

export function apply(choice: ThemeChoice) {
  const html = document.documentElement;
  if (choice === 'auto') {
    html.removeAttribute('data-theme');
  } else {
    html.setAttribute('data-theme', choice);
  }
}

export function setChoice(choice: ThemeChoice) {
  writeChoice(choice);
  apply(choice);
}
