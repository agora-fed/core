<script lang="ts">
  // Text input with optional label, hint, error, leading/trailing icon slot.
  // Two-way bind via `value`. Accessible: label ties to input, aria-describedby
  // wires the hint/error automatically.
  import type { Snippet } from 'svelte';

  interface Props {
    id?: string;
    label?: string;
    value?: string;
    type?:
      | 'text'
      | 'email'
      | 'password'
      | 'search'
      | 'tel'
      | 'url'
      | 'number';
    placeholder?: string;
    hint?: string;
    error?: string;
    required?: boolean;
    disabled?: boolean;
    autocomplete?: string;
    autofocus?: boolean;
    maxlength?: number;
    inputmode?:
      | 'text'
      | 'email'
      | 'search'
      | 'tel'
      | 'url'
      | 'numeric'
      | 'decimal'
      | 'none';
    leading?: Snippet;
    trailing?: Snippet;
    oninput?: (e: Event) => void;
    onchange?: (e: Event) => void;
    onblur?: (e: FocusEvent) => void;
  }

  let {
    id = crypto.randomUUID(),
    label,
    value = $bindable(''),
    type = 'text',
    placeholder,
    hint,
    error,
    required = false,
    disabled = false,
    autocomplete,
    autofocus = false,
    maxlength,
    inputmode,
    leading,
    trailing,
    oninput,
    onchange,
    onblur,
  }: Props = $props();

  const hintId = `${id}-hint`;
  const errorId = `${id}-err`;
  const describedBy = $derived(
    [error ? errorId : null, hint ? hintId : null].filter(Boolean).join(' ') ||
      undefined,
  );
</script>

<div class="field">
  {#if label}
    <label for={id}>
      {label}
      {#if required}<span class="req" aria-hidden="true">*</span>{/if}
    </label>
  {/if}
  <div class="wrap" class:invalid={!!error} class:disabled>
    {#if leading}
      <span class="affix affix-l">{@render leading()}</span>
    {/if}
    <input
      {id}
      {type}
      {placeholder}
      {required}
      {disabled}
      {autocomplete}
      {autofocus}
      {maxlength}
      {inputmode}
      aria-invalid={!!error}
      aria-describedby={describedBy}
      bind:value
      {oninput}
      {onchange}
      {onblur}
    />
    {#if trailing}
      <span class="affix affix-r">{@render trailing()}</span>
    {/if}
  </div>
  {#if error}
    <p class="hint hint-error" id={errorId}>{error}</p>
  {:else if hint}
    <p class="hint" id={hintId}>{hint}</p>
  {/if}
</div>

<style>
  .field {
    display: block;
    margin-bottom: var(--sp-4);
  }
  label {
    display: block;
    font-weight: var(--fw-semibold);
    font-size: var(--fs-sm);
    margin-bottom: var(--sp-1);
    color: var(--text-1);
  }
  .req {
    color: var(--danger);
    margin-left: 2px;
  }
  .wrap {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 0 var(--sp-3);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    transition:
      border-color var(--dur-fast) var(--ease-out),
      box-shadow var(--dur-fast) var(--ease-out);
  }
  .wrap:focus-within {
    border-color: var(--accent);
    box-shadow: var(--shadow-focus);
  }
  .wrap.invalid {
    border-color: var(--danger);
  }
  .wrap.invalid:focus-within {
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--danger) 35%, transparent);
  }
  .wrap.disabled {
    opacity: 0.6;
    pointer-events: none;
  }
  .affix {
    display: inline-flex;
    align-items: center;
    color: var(--text-3);
    flex-shrink: 0;
  }
  input {
    flex: 1;
    min-width: 0;
    background: transparent;
    border: 0;
    font: inherit;
    color: var(--text-1);
    padding: var(--sp-3) 0;
    outline: none;
  }
  input::placeholder {
    color: var(--text-3);
  }
  .hint {
    font-size: var(--fs-xs);
    margin: var(--sp-1) 0 0;
    color: var(--text-3);
  }
  .hint-error {
    color: var(--danger);
  }
</style>
