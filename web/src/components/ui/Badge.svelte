<script lang="ts">
  import type { Snippet } from 'svelte';
  type Tone =
    | 'neutral'
    | 'accent'
    | 'success'
    | 'warning'
    | 'danger'
    | 'info'
    | 'pending'
    | 'answered'
    | 'acted'
    | 'ignored';
  interface Props {
    tone?: Tone;
    size?: 'sm' | 'base';
    outline?: boolean;
    children: Snippet;
  }
  let {
    tone = 'neutral',
    size = 'base',
    outline = false,
    children,
  }: Props = $props();
</script>

<span class={`b t-${tone} s-${size}`} class:outline>
  {@render children()}
</span>

<style>
  .b {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    border-radius: var(--r-full);
    border: 1px solid transparent;
    font-weight: var(--fw-semibold);
    line-height: 1.15;
    white-space: nowrap;
  }
  .s-sm {
    padding: 2px var(--sp-2);
    font-size: var(--fs-xs);
  }
  .s-base {
    padding: var(--sp-1) var(--sp-3);
    font-size: var(--fs-xs);
  }
  /* Filled tones (default) */
  .t-neutral {
    color: var(--text-2);
    background: var(--surface-3);
  }
  .t-accent {
    color: var(--accent-strong);
    background: var(--accent-soft);
  }
  .t-success,
  .t-answered {
    color: var(--success);
    background: var(--success-soft);
  }
  .t-warning,
  .t-pending {
    color: var(--warning);
    background: var(--warning-soft);
  }
  .t-danger,
  .t-ignored {
    color: var(--danger);
    background: var(--danger-soft);
  }
  .t-info,
  .t-acted {
    color: var(--info);
    background: var(--info-soft);
  }
  /* Outline modifier */
  .outline {
    background: transparent;
    border-color: currentColor;
  }
</style>
