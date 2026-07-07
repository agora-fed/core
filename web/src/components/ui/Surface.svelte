<script lang="ts">
  import type { Snippet } from 'svelte';
  interface Props {
    layer?: 0 | 1 | 2 | 3;
    class?: string;
    children: Snippet;
  }
  let { layer = 1, class: klass = '', children }: Props = $props();
</script>

<div class={`s l-${layer} ${klass}`}>
  {@render children()}
</div>

<style>
  .s {
    color: var(--text-1);
    transition:
      background var(--dur-base) var(--ease-out),
      color var(--dur-base) var(--ease-out);
  }
  .l-0 {
    background: var(--surface-0);
  }
  .l-1 {
    background: var(--surface-1);
  }
  .l-2 {
    background: var(--surface-2);
  }
  .l-3 {
    background: var(--surface-3);
  }
</style>
