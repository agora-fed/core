<script lang="ts">
  // ToastHost: single instance mounted in AppShell. Subscribes to the toasts
  // store and renders the queue in a fixed corner. Individual toasts have
  // a self-dismiss via the store's TTL.
  import { toasts, dismiss, type ToastTone } from '../../lib/toasts';
  import Icon from './Icon.svelte';

  const iconFor: Record<ToastTone, string> = {
    success: 'check',
    error: 'alert',
    info: 'info',
    warning: 'alert',
  };
</script>

<div class="host" aria-live="polite" aria-atomic="true">
  {#each $toasts as t (t.id)}
    <div class={`t t-${t.tone}`} role="status">
      <span class="ico"><Icon name={iconFor[t.tone]} size={18} /></span>
      <div class="body">
        {#if t.title}<strong>{t.title}</strong>{/if}
        <span>{t.message}</span>
      </div>
      <button
        type="button"
        aria-label="Fechar"
        class="close"
        onclick={() => dismiss(t.id)}
      >
        <Icon name="x" size={16} />
      </button>
    </div>
  {/each}
</div>

<style>
  .host {
    position: fixed;
    bottom: var(--sp-4);
    right: var(--sp-4);
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    z-index: 120;
    max-width: calc(100vw - var(--sp-8));
    pointer-events: none;
  }
  .t {
    display: flex;
    align-items: flex-start;
    gap: var(--sp-3);
    background: var(--surface-1);
    color: var(--text-1);
    border: 1px solid var(--border-subtle);
    border-left: 4px solid var(--accent);
    border-radius: var(--r-base);
    padding: var(--sp-3) var(--sp-4);
    box-shadow: var(--shadow-lg);
    min-width: 16rem;
    max-width: 24rem;
    pointer-events: auto;
    animation: slide-in var(--dur-base) var(--ease-out);
  }
  .t-success {
    border-left-color: var(--success);
  }
  .t-error {
    border-left-color: var(--danger);
  }
  .t-info {
    border-left-color: var(--info);
  }
  .t-warning {
    border-left-color: var(--warning);
  }
  .ico {
    display: inline-flex;
    padding-top: 2px;
  }
  .t-success .ico {
    color: var(--success);
  }
  .t-error .ico {
    color: var(--danger);
  }
  .t-info .ico {
    color: var(--info);
  }
  .t-warning .ico {
    color: var(--warning);
  }
  .body {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: var(--fs-sm);
  }
  .body strong {
    color: var(--text-1);
    font-weight: var(--fw-semibold);
  }
  .body span {
    color: var(--text-2);
    line-height: var(--lh-snug);
  }
  .close {
    background: transparent;
    border: 0;
    color: var(--text-3);
    cursor: pointer;
    padding: 2px;
    border-radius: var(--r-xs);
  }
  .close:hover {
    color: var(--text-1);
    background: var(--surface-2);
  }
  @keyframes slide-in {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
</style>
