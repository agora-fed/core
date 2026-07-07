<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Tab {
    id: string;
    label: string;
    count?: number;
  }

  interface Props {
    tabs: Tab[];
    active: string;
    onselect?: (id: string) => void;
    children?: Snippet<[string]>;
  }
  let { tabs, active = $bindable(), onselect, children }: Props = $props();

  function pick(id: string) {
    active = id;
    onselect?.(id);
  }
</script>

<div class="tabs" role="tablist">
  {#each tabs as t}
    <button
      type="button"
      role="tab"
      class:active={active === t.id}
      aria-selected={active === t.id}
      onclick={() => pick(t.id)}
    >
      <span>{t.label}</span>
      {#if t.count !== undefined}<span class="count">{t.count}</span>{/if}
    </button>
  {/each}
</div>
{#if children}
  <div role="tabpanel">
    {@render children(active)}
  </div>
{/if}

<style>
  .tabs {
    display: flex;
    gap: var(--sp-1);
    border-bottom: 1px solid var(--border-subtle);
    overflow-x: auto;
    scrollbar-width: none;
  }
  .tabs::-webkit-scrollbar {
    display: none;
  }
  button {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-3) var(--sp-4);
    background: transparent;
    border: 0;
    border-bottom: 2px solid transparent;
    font: inherit;
    font-size: var(--fs-sm);
    font-weight: var(--fw-medium);
    color: var(--text-3);
    cursor: pointer;
    white-space: nowrap;
    transition:
      color var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out);
    margin-bottom: -1px;
  }
  button:hover {
    color: var(--text-1);
  }
  button.active {
    color: var(--accent);
    border-bottom-color: var(--accent);
    font-weight: var(--fw-semibold);
  }
  button:focus-visible {
    outline: none;
    color: var(--accent);
    box-shadow: var(--shadow-focus);
    border-radius: var(--r-sm) var(--r-sm) 0 0;
  }
  .count {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 20px;
    padding: 2px 6px;
    font-size: var(--fs-xs);
    background: var(--surface-2);
    border-radius: var(--r-full);
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
  button.active .count {
    background: var(--accent-soft);
    color: var(--accent-strong);
  }
</style>
