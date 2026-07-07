<script lang="ts">
  // Button primitive. Renders <button> or <a> depending on `href`.
  // Variants map to semantic tokens; sizes follow the 8px scale.
  import type { Snippet } from 'svelte';

  type Variant = 'primary' | 'secondary' | 'ghost' | 'danger' | 'subtle';
  type Size = 'sm' | 'base' | 'lg';

  interface Props {
    variant?: Variant;
    size?: Size;
    href?: string;
    type?: 'button' | 'submit' | 'reset';
    disabled?: boolean;
    loading?: boolean;
    fullWidth?: boolean;
    ariaLabel?: string;
    title?: string;
    onclick?: (e: MouseEvent) => void;
    children: Snippet;
  }

  let {
    variant = 'primary',
    size = 'base',
    href,
    type = 'button',
    disabled = false,
    loading = false,
    fullWidth = false,
    ariaLabel,
    title,
    onclick,
    children,
  }: Props = $props();
</script>

{#if href}
  <a
    class={`btn v-${variant} s-${size}`}
    class:full={fullWidth}
    class:loading
    aria-disabled={disabled || loading}
    aria-label={ariaLabel}
    {title}
    {href}
    onclick={disabled || loading ? (e) => e.preventDefault() : onclick}
  >
    {@render children()}
  </a>
{:else}
  <button
    class={`btn v-${variant} s-${size}`}
    class:full={fullWidth}
    class:loading
    {type}
    disabled={disabled || loading}
    aria-label={ariaLabel}
    {title}
    {onclick}
  >
    {@render children()}
  </button>
{/if}

<style>
  .btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: var(--sp-2);
    font: inherit;
    font-weight: var(--fw-semibold);
    border-radius: var(--r-full);
    border: 1px solid transparent;
    cursor: pointer;
    text-decoration: none;
    text-align: center;
    max-width: 100%;
    white-space: nowrap;
    transition:
      background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out),
      box-shadow var(--dur-fast) var(--ease-out),
      transform var(--dur-instant) var(--ease-out);
  }
  .btn:active:not(.loading):not([disabled]) {
    transform: translateY(1px);
  }
  .btn:focus-visible {
    outline: none;
    box-shadow: var(--shadow-focus);
  }
  .btn[disabled],
  .btn[aria-disabled='true'] {
    opacity: 0.55;
    cursor: not-allowed;
  }
  .btn.full {
    width: 100%;
  }
  .btn.loading {
    color: transparent;
    position: relative;
  }
  .btn.loading::after {
    content: '';
    position: absolute;
    inset: 0;
    margin: auto;
    width: 1em;
    height: 1em;
    border: 2px solid currentColor;
    border-color: transparent;
    border-top-color: var(--accent-contrast);
    border-radius: 50%;
    animation: btn-spin 0.7s linear infinite;
    color: initial;
  }
  @keyframes btn-spin {
    to {
      transform: rotate(1turn);
    }
  }

  /* Sizes */
  .s-sm {
    padding: var(--sp-2) var(--sp-4);
    font-size: var(--fs-sm);
  }
  .s-base {
    padding: var(--sp-3) var(--sp-6);
    font-size: var(--fs-md);
  }
  .s-lg {
    padding: var(--sp-4) var(--sp-8);
    font-size: var(--fs-lg);
  }

  /* Variants */
  .v-primary {
    background: var(--accent);
    color: var(--accent-contrast);
  }
  .v-primary:hover:not(.loading):not([disabled]) {
    background: var(--accent-strong);
  }
  .v-secondary {
    background: var(--surface-2);
    color: var(--text-1);
    border-color: var(--border-subtle);
  }
  .v-secondary:hover:not(.loading):not([disabled]) {
    background: var(--surface-3);
  }
  .v-ghost {
    background: transparent;
    color: var(--text-1);
    border-color: var(--border-subtle);
  }
  .v-ghost:hover:not(.loading):not([disabled]) {
    background: var(--surface-2);
  }
  .v-subtle {
    background: transparent;
    color: var(--text-2);
    border-color: transparent;
  }
  .v-subtle:hover:not(.loading):not([disabled]) {
    background: var(--surface-2);
    color: var(--text-1);
  }
  .v-danger {
    background: var(--danger);
    color: var(--text-inverse);
  }
  .v-danger:hover:not(.loading):not([disabled]) {
    filter: brightness(0.92);
  }
</style>
