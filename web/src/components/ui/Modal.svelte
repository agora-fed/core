<script lang="ts">
  // Modal: renders a fixed-position dialog with backdrop. Focus is captured on
  // open (first focusable element), returned on close. Escape + backdrop click
  // dismiss unless `dismissable={false}`.
  import { onMount, tick } from 'svelte';
  import type { Snippet } from 'svelte';

  interface Props {
    open: boolean;
    title?: string;
    dismissable?: boolean;
    size?: 'sm' | 'base' | 'lg';
    onclose?: () => void;
    header?: Snippet;
    children: Snippet;
    footer?: Snippet;
  }

  let {
    open = $bindable(),
    title,
    dismissable = true,
    size = 'base',
    onclose,
    header,
    children,
    footer,
  }: Props = $props();

  let dialog = $state<HTMLDivElement | null>(null);
  let previouslyFocused: HTMLElement | null = null;

  $effect(() => {
    if (open) {
      previouslyFocused = document.activeElement as HTMLElement | null;
      document.body.style.overflow = 'hidden';
      tick().then(() => {
        const el = dialog?.querySelector<HTMLElement>(
          '[autofocus], button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
        );
        el?.focus();
      });
    } else {
      document.body.style.overflow = '';
      previouslyFocused?.focus();
    }
  });

  function close() {
    if (!dismissable) return;
    open = false;
    onclose?.();
  }

  function onKey(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === 'Escape') close();
    // Simple focus trap: contain tab within the dialog.
    if (e.key === 'Tab' && dialog) {
      const focusables = dialog.querySelectorAll<HTMLElement>(
        'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
      );
      if (focusables.length === 0) return;
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if open}
  <div
    class="backdrop"
    role="presentation"
    onclick={close}
    onkeydown={(e) => e.key === 'Enter' && close()}
    tabindex="-1"
  >
    <div
      bind:this={dialog}
      class={`dialog s-${size}`}
      role="dialog"
      aria-modal="true"
      aria-label={title}
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
      tabindex="-1"
    >
      {#if header}
        <div class="head">{@render header()}</div>
      {:else if title}
        <div class="head">
          <h2>{title}</h2>
          {#if dismissable}
            <button
              type="button"
              class="close"
              aria-label="Fechar"
              onclick={close}
            >
              ×
            </button>
          {/if}
        </div>
      {/if}
      <div class="body">
        {@render children()}
      </div>
      {#if footer}
        <div class="foot">{@render footer()}</div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, var(--surface-inverse) 55%, transparent);
    backdrop-filter: blur(2px);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--sp-4);
    z-index: 100;
    animation: fade var(--dur-fast) var(--ease-out);
  }
  .dialog {
    background: var(--surface-1);
    color: var(--text-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-xl);
    width: 100%;
    max-height: calc(100vh - var(--sp-8));
    display: flex;
    flex-direction: column;
    animation: rise var(--dur-base) var(--ease-out);
  }
  .s-sm {
    max-width: 24rem;
  }
  .s-base {
    max-width: 32rem;
  }
  .s-lg {
    max-width: 48rem;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-4);
    padding: var(--sp-5) var(--sp-6);
    border-bottom: 1px solid var(--border-subtle);
  }
  .head h2 {
    margin: 0;
    font-size: var(--fs-xl);
  }
  .close {
    background: transparent;
    border: 0;
    font-size: 1.5rem;
    line-height: 1;
    cursor: pointer;
    color: var(--text-3);
    padding: 0 var(--sp-2);
    border-radius: var(--r-sm);
  }
  .close:hover {
    color: var(--text-1);
    background: var(--surface-2);
  }
  .body {
    padding: var(--sp-6);
    overflow-y: auto;
  }
  .foot {
    display: flex;
    justify-content: flex-end;
    gap: var(--sp-3);
    padding: var(--sp-4) var(--sp-6);
    border-top: 1px solid var(--border-subtle);
    background: var(--surface-2);
    border-bottom-left-radius: var(--r-lg);
    border-bottom-right-radius: var(--r-lg);
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(8px) scale(0.98);
    }
    to {
      opacity: 1;
      transform: translateY(0) scale(1);
    }
  }
</style>
