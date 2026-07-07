<script lang="ts">
  // Compact tri-state theme picker. Reads current choice from localStorage on
  // mount, keeps html[data-theme] in sync. Used in the header (desktop menu
  // and mobile drawer). Uses raw SVGs inline — the icon sprite is introduced
  // separately in a later component pass, so this island stays standalone.
  import { onMount } from 'svelte';
  import { readChoice, setChoice, type ThemeChoice } from '../../lib/theme';

  let choice = $state<ThemeChoice>('auto');
  let ready = $state(false);

  onMount(() => {
    choice = readChoice();
    ready = true;
  });

  function pick(next: ThemeChoice) {
    choice = next;
    setChoice(next);
  }
</script>

{#if ready}
  <div class="wrap" role="group" aria-label="Tema visual">
    <button
      type="button"
      class:active={choice === 'auto'}
      onclick={() => pick('auto')}
      title="Seguir sistema"
      aria-pressed={choice === 'auto'}
    >
      <svg viewBox="0 0 20 20" width="16" height="16" aria-hidden="true">
        <path
          d="M10 3v14M4 10h12"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          fill="none"
        />
        <circle cx="10" cy="10" r="7" stroke="currentColor" stroke-width="1.8" fill="none" />
      </svg>
      <span class="lbl">Auto</span>
    </button>
    <button
      type="button"
      class:active={choice === 'light'}
      onclick={() => pick('light')}
      title="Tema claro"
      aria-pressed={choice === 'light'}
    >
      <svg viewBox="0 0 20 20" width="16" height="16" aria-hidden="true">
        <circle cx="10" cy="10" r="3.6" fill="currentColor" />
        <g stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
          <path d="M10 2v2M10 16v2M2 10h2M16 10h2M4.2 4.2l1.4 1.4M14.4 14.4l1.4 1.4M4.2 15.8l1.4-1.4M14.4 5.6l1.4-1.4" />
        </g>
      </svg>
      <span class="lbl">Claro</span>
    </button>
    <button
      type="button"
      class:active={choice === 'dark'}
      onclick={() => pick('dark')}
      title="Tema escuro"
      aria-pressed={choice === 'dark'}
    >
      <svg viewBox="0 0 20 20" width="16" height="16" aria-hidden="true">
        <path
          d="M15.5 12.5a6 6 0 0 1-8-8 6 6 0 1 0 8 8Z"
          fill="currentColor"
        />
      </svg>
      <span class="lbl">Escuro</span>
    </button>
  </div>
{/if}

<style>
  .wrap {
    display: inline-flex;
    gap: 2px;
    padding: 3px;
    background: var(--surface-2);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-full);
  }
  button {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    padding: var(--sp-1) var(--sp-3);
    border: 0;
    border-radius: var(--r-full);
    background: transparent;
    color: var(--text-3);
    font: inherit;
    font-size: var(--fs-xs);
    font-weight: var(--fw-semibold);
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  button:hover {
    color: var(--text-1);
  }
  button.active {
    background: var(--surface-1);
    color: var(--text-1);
    box-shadow: var(--shadow-sm);
  }
  button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .lbl {
    line-height: 1;
  }
  @media (max-width: 520px) {
    .lbl {
      /* icon-only on narrow drawers */
      display: none;
    }
  }
</style>
