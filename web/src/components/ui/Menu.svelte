<script lang="ts">
  // Dropdown menu with trigger + items. Focus loops with arrow keys; Escape
  // and outside-click close. Consumers pass items via the `items` snippet
  // (usually a list of <a> or <button>) and the trigger via `trigger`.
  import type { Snippet } from 'svelte';

  interface Props {
    align?: 'left' | 'right';
    label?: string;
    trigger: Snippet<[{ open: boolean; toggle: () => void }]>;
    items: Snippet;
  }

  let { align = 'right', label, trigger, items }: Props = $props();

  let open = $state(false);
  let root = $state<HTMLDivElement | null>(null);
  let menuEl = $state<HTMLDivElement | null>(null);

  function toggle() {
    open = !open;
  }

  function onDoc(e: MouseEvent) {
    if (open && root && !root.contains(e.target as Node)) open = false;
  }

  function onKey(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === 'Escape') {
      open = false;
      return;
    }
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const list = menuEl?.querySelectorAll<HTMLElement>(
        'a, button, [tabindex]:not([tabindex="-1"])',
      );
      if (!list || list.length === 0) return;
      const current = document.activeElement as HTMLElement | null;
      const idx = Array.from(list).indexOf(current!);
      const next =
        e.key === 'ArrowDown'
          ? list[(idx + 1) % list.length]
          : list[(idx - 1 + list.length) % list.length];
      next?.focus();
    }
  }
</script>

<svelte:window onclick={onDoc} onkeydown={onKey} />

<div class="root" bind:this={root}>
  {@render trigger({ open, toggle })}
  {#if open}
    <div
      bind:this={menuEl}
      class={`menu a-${align}`}
      role="menu"
      aria-label={label}
    >
      {@render items()}
    </div>
  {/if}
</div>

<style>
  .root {
    position: relative;
    display: inline-flex;
  }
  .menu {
    position: absolute;
    top: calc(100% + 6px);
    min-width: 12rem;
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    box-shadow: var(--shadow-lg);
    padding: var(--sp-1);
    display: flex;
    flex-direction: column;
    z-index: 60;
    animation: rise var(--dur-fast) var(--ease-out);
  }
  .a-right {
    right: 0;
  }
  .a-left {
    left: 0;
  }
  :global(.menu > a),
  :global(.menu > button) {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-3);
    border-radius: var(--r-sm);
    text-decoration: none;
    color: var(--text-1);
    font: inherit;
    font-size: var(--fs-sm);
    font-weight: var(--fw-medium);
    text-align: left;
    background: transparent;
    border: 0;
    cursor: pointer;
  }
  :global(.menu > a:hover),
  :global(.menu > button:hover),
  :global(.menu > a:focus-visible),
  :global(.menu > button:focus-visible) {
    background: var(--surface-2);
    outline: none;
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(-4px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
</style>
