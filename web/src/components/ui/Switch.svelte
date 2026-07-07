<script lang="ts">
  interface Props {
    checked?: boolean;
    disabled?: boolean;
    label?: string;
    onchange?: (checked: boolean) => void;
  }
  let {
    checked = $bindable(false),
    disabled = false,
    label,
    onchange,
  }: Props = $props();

  function toggle() {
    if (disabled) return;
    checked = !checked;
    onchange?.(checked);
  }
</script>

<label class="sw" class:on={checked} class:disabled>
  <button
    type="button"
    role="switch"
    aria-checked={checked}
    aria-label={label}
    {disabled}
    onclick={toggle}
  >
    <span class="thumb"></span>
  </button>
  {#if label}<span class="lbl">{label}</span>{/if}
</label>

<style>
  .sw {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
    cursor: pointer;
  }
  .sw.disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }
  button {
    position: relative;
    width: 40px;
    height: 22px;
    border-radius: var(--r-full);
    background: var(--surface-3);
    border: 0;
    cursor: pointer;
    padding: 0;
    transition: background var(--dur-fast) var(--ease-out);
  }
  .thumb {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 18px;
    height: 18px;
    background: var(--surface-1);
    border-radius: 50%;
    box-shadow: var(--shadow-sm);
    transition: transform var(--dur-fast) var(--ease-out);
  }
  .on button {
    background: var(--accent);
  }
  .on .thumb {
    transform: translateX(18px);
  }
  button:focus-visible {
    outline: none;
    box-shadow: var(--shadow-focus);
  }
  .lbl {
    font-size: var(--fs-sm);
    color: var(--text-2);
  }
</style>
