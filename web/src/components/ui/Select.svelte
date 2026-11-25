<script lang="ts">
  // Native <select> with the same look as Input: label, hint, focus ring, dark mode.
  // Options may carry a `group` to render <optgroup>s, in first-seen order.
  export interface SelectOption {
    value: string;
    label: string;
    group?: string;
  }

  interface Props {
    id?: string;
    label: string;
    /** Hide the label visually but keep it for screen readers. */
    hideLabel?: boolean;
    value?: string;
    options: SelectOption[];
    hint?: string;
    disabled?: boolean;
    compact?: boolean;
    onchange?: (value: string) => void;
  }

  let {
    id = crypto.randomUUID(),
    label,
    hideLabel = false,
    value = $bindable(''),
    options,
    hint,
    disabled = false,
    compact = false,
    onchange,
  }: Props = $props();

  const grouped = $derived.by(() => {
    const out: { group: string; options: SelectOption[] }[] = [];
    for (const o of options) {
      const g = o.group ?? '';
      let bucket = out.find((b) => b.group === g);
      if (!bucket) {
        bucket = { group: g, options: [] };
        out.push(bucket);
      }
      bucket.options.push(o);
    }
    return out;
  });
</script>

<div class="field" class:compact>
  <label for={id} class:sr-only={hideLabel}>{label}</label>
  <div class="wrap" class:disabled>
    <select
      {id}
      {disabled}
      bind:value
      aria-describedby={hint ? `${id}-hint` : undefined}
      onchange={() => onchange?.(value)}
    >
      {#each grouped as bucket (bucket.group)}
        {#if bucket.group}
          <optgroup label={bucket.group}>
            {#each bucket.options as o (o.value)}
              <option value={o.value}>{o.label}</option>
            {/each}
          </optgroup>
        {:else}
          {#each bucket.options as o (o.value)}
            <option value={o.value}>{o.label}</option>
          {/each}
        {/if}
      {/each}
    </select>
  </div>
  {#if hint}<p class="hint" id={`${id}-hint`}>{hint}</p>{/if}
</div>

<style>
  .field {
    display: block;
    margin-bottom: var(--sp-4);
    min-width: 0;
  }
  .field.compact {
    margin-bottom: 0;
  }
  label {
    display: block;
    font-weight: var(--fw-semibold);
    font-size: var(--fs-sm);
    margin-bottom: var(--sp-1);
    color: var(--text-1);
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }
  .wrap {
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
  .wrap.disabled {
    opacity: 0.6;
  }
  select {
    width: 100%;
    min-width: 0;
    /* Same box as Input: inherited font, --sp-3 vertical padding. */
    padding: var(--sp-3);
    border: 0;
    background: transparent;
    color: var(--text-1);
    font: inherit;
    text-overflow: ellipsis;
    outline: none;
    cursor: pointer;
  }
  select option,
  select optgroup {
    background: var(--surface-1);
    color: var(--text-1);
  }
  .hint {
    margin: var(--sp-1) 0 0;
    font-size: var(--fs-xs);
    color: var(--text-3);
  }
</style>
