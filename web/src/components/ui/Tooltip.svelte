<script lang="ts">
  // Tooltip: CSS-only via a wrapper element; content appears on hover/focus.
  // Uses the `title` fallback for the pointer-only case is intentionally NOT
  // added — the visual tip is the primary affordance. Keep text short.
  import type { Snippet } from 'svelte';
  interface Props {
    text: string;
    placement?: 'top' | 'bottom';
    children: Snippet;
  }
  let { text, placement = 'top', children }: Props = $props();
</script>

<span class={`tip p-${placement}`} data-tip={text}>
  {@render children()}
</span>

<style>
  .tip {
    position: relative;
    display: inline-flex;
  }
  .tip::after {
    content: attr(data-tip);
    position: absolute;
    left: 50%;
    transform: translateX(-50%) translateY(0);
    background: var(--surface-inverse);
    color: var(--text-inverse);
    font-size: var(--fs-xs);
    font-weight: var(--fw-medium);
    padding: var(--sp-1) var(--sp-2);
    border-radius: var(--r-xs);
    white-space: nowrap;
    opacity: 0;
    pointer-events: none;
    transition:
      opacity var(--dur-fast) var(--ease-out),
      transform var(--dur-fast) var(--ease-out);
    z-index: 40;
  }
  .p-top::after {
    bottom: calc(100% + 6px);
  }
  .p-bottom::after {
    top: calc(100% + 6px);
  }
  .tip:hover::after,
  .tip:focus-within::after {
    opacity: 1;
    transform: translateX(-50%) translateY(-2px);
  }
  .p-bottom:hover::after,
  .p-bottom:focus-within::after {
    transform: translateX(-50%) translateY(2px);
  }
</style>
