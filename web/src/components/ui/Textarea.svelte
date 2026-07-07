<script lang="ts">
  interface Props {
    id?: string;
    label?: string;
    value?: string;
    placeholder?: string;
    hint?: string;
    error?: string;
    required?: boolean;
    disabled?: boolean;
    rows?: number;
    maxlength?: number;
    autoResize?: boolean;
    oninput?: (e: Event) => void;
    onkeydown?: (e: KeyboardEvent) => void;
    onselect?: (e: Event) => void;
    /** Bindable ref to the underlying <textarea> — needed for caret-aware
     *  features like autocomplete in NoteComposer. */
    element?: HTMLTextAreaElement | null;
  }

  let {
    id = crypto.randomUUID(),
    label,
    value = $bindable(''),
    placeholder,
    hint,
    error,
    required = false,
    disabled = false,
    rows = 4,
    maxlength,
    autoResize = false,
    oninput,
    onkeydown,
    onselect,
    element = $bindable(null),
  }: Props = $props();

  let ref = $state<HTMLTextAreaElement | null>(null);
  $effect(() => {
    element = ref;
  });
  const hintId = `${id}-hint`;
  const errorId = `${id}-err`;

  $effect(() => {
    if (!autoResize || !ref) return;
    ref.style.height = 'auto';
    ref.style.height = `${ref.scrollHeight}px`;
  });

  const count = $derived(value?.length ?? 0);
</script>

<div class="field">
  {#if label}
    <label for={id}>
      {label}
      {#if required}<span class="req" aria-hidden="true">*</span>{/if}
    </label>
  {/if}
  <textarea
    bind:this={ref}
    {id}
    {rows}
    {placeholder}
    {required}
    {disabled}
    {maxlength}
    aria-invalid={!!error}
    aria-describedby={error ? errorId : hint ? hintId : undefined}
    bind:value
    {oninput}
    {onkeydown}
    {onselect}
  ></textarea>
  <div class="foot">
    {#if error}
      <p class="hint hint-error" id={errorId}>{error}</p>
    {:else if hint}
      <p class="hint" id={hintId}>{hint}</p>
    {:else}
      <span></span>
    {/if}
    {#if maxlength}
      <p class="counter" class:near={count > maxlength * 0.9}>
        {count}/{maxlength}
      </p>
    {/if}
  </div>
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
  textarea {
    width: 100%;
    resize: vertical;
    font: inherit;
    padding: var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-1);
    line-height: var(--lh-base);
    transition:
      border-color var(--dur-fast) var(--ease-out),
      box-shadow var(--dur-fast) var(--ease-out);
  }
  textarea:focus-visible {
    outline: none;
    border-color: var(--accent);
    box-shadow: var(--shadow-focus);
  }
  textarea[aria-invalid='true'] {
    border-color: var(--danger);
  }
  .foot {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--sp-2);
    margin-top: var(--sp-1);
  }
  .hint {
    font-size: var(--fs-xs);
    margin: 0;
    color: var(--text-3);
  }
  .hint-error {
    color: var(--danger);
  }
  .counter {
    font-size: var(--fs-xs);
    color: var(--text-3);
    margin: 0;
    font-variant-numeric: tabular-nums;
  }
  .counter.near {
    color: var(--warning);
    font-weight: var(--fw-semibold);
  }
</style>
