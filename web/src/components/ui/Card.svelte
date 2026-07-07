<script lang="ts">
  import type { Snippet } from 'svelte';
  interface Props {
    padding?: 'sm' | 'base' | 'lg' | 'none';
    interactive?: boolean;
    class?: string;
    as?: 'div' | 'article' | 'section';
    children: Snippet;
  }
  let {
    padding = 'base',
    interactive = false,
    class: klass = '',
    as = 'div',
    children,
  }: Props = $props();
</script>

<svelte:element
  this={as}
  class={`card p-${padding} ${interactive ? 'is-interactive' : ''} ${klass}`}
>
  {@render children()}
</svelte:element>

<style>
  .card {
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    box-shadow: var(--shadow-sm);
    overflow-wrap: break-word;
    min-width: 0;
    transition:
      box-shadow var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out);
  }
  .p-none {
    padding: 0;
  }
  .p-sm {
    padding: var(--sp-3);
  }
  .p-base {
    padding: var(--sp-5);
  }
  .p-lg {
    padding: var(--sp-8);
  }
  .is-interactive {
    cursor: pointer;
  }
  .is-interactive:hover {
    border-color: var(--border-strong);
    box-shadow: var(--shadow-base);
  }
</style>
