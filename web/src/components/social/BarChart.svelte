<script lang="ts">
  // Minimal horizontal bar chart. Renders one bar per data row, sorted
  // descending by value (already sorted server-side). Values normalize
  // against the max so the widest bar fills the row.
  //
  // Kept dependency-free — no D3, no ApexCharts. The number after the bar
  // is the "raw number" the user asked for.
  interface Row {
    label: string;
    value: number;
    /** Optional secondary count shown after the value (e.g. mandate count). */
    hint?: string;
    /** Optional accent override (defaults to --accent). */
    color?: string;
  }
  interface Props {
    rows: Row[];
    /** Formatter for the numeric value — e.g. R$ or "N propostas". */
    format?: (v: number) => string;
    max?: number;
    empty?: string;
  }
  let {
    rows,
    format = (v: number) => v.toLocaleString('pt-BR'),
    max,
    empty = 'Sem dados para os filtros escolhidos.',
  }: Props = $props();

  const computedMax = $derived(
    max ?? Math.max(1, ...rows.map((r) => r.value)),
  );
</script>

{#if rows.length === 0}
  <p class="empty">{empty}</p>
{:else}
  <ol class="bars" aria-label="Distribuição por categoria">
    {#each rows as r, i (r.label + '::' + i)}
      {@const pct = Math.max(2, Math.round((r.value / computedMax) * 100))}
      <li>
        <div class="label" title={r.label}>{r.label}</div>
        <div class="track" role="img" aria-label={`${r.label}: ${format(r.value)}`}>
          <div
            class="fill"
            style={`--pct:${pct}%; --bar-color:${r.color ?? 'var(--accent)'}`}
          ></div>
        </div>
        <div class="value">
          <strong>{format(r.value)}</strong>
          {#if r.hint}<span class="hint">{r.hint}</span>{/if}
        </div>
      </li>
    {/each}
  </ol>
{/if}

<style>
  .bars {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-2);
  }
  .bars li {
    display: grid;
    grid-template-columns: minmax(120px, 22%) 1fr minmax(140px, auto);
    align-items: center;
    gap: var(--sp-3);
  }
  .label {
    font-size: var(--fs-sm);
    color: var(--text-1);
    font-weight: var(--fw-medium);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .track {
    height: 24px;
    background: var(--surface-2);
    border-radius: var(--r-sm);
    overflow: hidden;
    position: relative;
  }
  .fill {
    height: 100%;
    width: var(--pct, 0%);
    background: var(--bar-color, var(--accent));
    border-radius: var(--r-sm);
    transition: width var(--dur-base) var(--ease-out);
  }
  .value {
    font-variant-numeric: tabular-nums;
    text-align: right;
    color: var(--text-1);
    font-size: var(--fs-sm);
  }
  .value strong {
    font-weight: var(--fw-semibold);
  }
  .value .hint {
    display: block;
    color: var(--text-3);
    font-size: var(--fs-xs);
    font-weight: var(--fw-medium);
  }
  .empty {
    color: var(--text-3);
    font-size: var(--fs-sm);
    text-align: center;
    padding: var(--sp-6) 0;
  }

  @media (max-width: 640px) {
    .bars li {
      grid-template-columns: 100px 1fr 90px;
    }
  }
</style>
