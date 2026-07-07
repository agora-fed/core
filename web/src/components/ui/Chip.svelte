<script lang="ts">
  import type { Snippet } from 'svelte';
  interface Props {
    selected?: boolean;
    interactive?: boolean;
    onclick?: () => void;
    children: Snippet;
  }
  let { selected = false, interactive = true, onclick, children }: Props =
    $props();
</script>

{#if interactive}
  <button
    type="button"
    class="chip"
    class:selected
    aria-pressed={selected}
    {onclick}
  >
    {@render children()}
  </button>
{:else}
  <span class="chip" class:selected>
    {@render children()}
  </span>
{/if}

<style>
  .chip {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    padding: var(--sp-1) var(--sp-3);
    border-radius: var(--r-full);
    background: var(--surface-2);
    color: var(--text-2);
    border: 1px solid var(--border-subtle);
    font: inherit;
    font-size: var(--fs-sm);
    font-weight: var(--fw-medium);
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out);
  }
  .chip:hover {
    background: var(--surface-3);
    color: var(--text-1);
  }
  .chip.selected {
    background: var(--accent-soft);
    color: var(--accent-strong);
    border-color: var(--accent);
  }
  .chip:focus-visible {
    outline: none;
    box-shadow: var(--shadow-focus);
  }
</style>
