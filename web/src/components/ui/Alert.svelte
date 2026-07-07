<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';

  type Tone = 'info' | 'success' | 'warning' | 'danger';
  interface Props {
    tone?: Tone;
    title?: string;
    children: Snippet;
  }
  let { tone = 'info', title, children }: Props = $props();

  const iconFor: Record<Tone, string> = {
    info: 'info',
    success: 'check',
    warning: 'alert',
    danger: 'alert',
  };
</script>

<div class={`a t-${tone}`} role="status">
  <span class="ico"><Icon name={iconFor[tone]} size={18} /></span>
  <div class="body">
    {#if title}<strong>{title}</strong>{/if}
    <div>{@render children()}</div>
  </div>
</div>

<style>
  .a {
    display: flex;
    align-items: flex-start;
    gap: var(--sp-3);
    padding: var(--sp-3) var(--sp-4);
    border-radius: var(--r-base);
    border: 1px solid transparent;
    font-size: var(--fs-sm);
    line-height: var(--lh-snug);
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }
  .body strong {
    font-weight: var(--fw-semibold);
  }
  .t-info {
    background: var(--info-soft);
    color: var(--info);
    border-color: color-mix(in srgb, var(--info) 20%, transparent);
  }
  .t-success {
    background: var(--success-soft);
    color: var(--success);
    border-color: color-mix(in srgb, var(--success) 20%, transparent);
  }
  .t-warning {
    background: var(--warning-soft);
    color: var(--warning);
    border-color: color-mix(in srgb, var(--warning) 20%, transparent);
  }
  .t-danger {
    background: var(--danger-soft);
    color: var(--danger);
    border-color: color-mix(in srgb, var(--danger) 20%, transparent);
  }
</style>
