<script lang="ts">
  // The count of one scenario: tiles, a bar in voting order, the threshold marker, the outcome.
  import { barSegments, orientationsText, outcomeText, prioritiesIn, undecidedId, type Scenario } from '../../lib/articulacao';

  let { scenario: sc, priorities }: { scenario: Scenario; priorities: number[] } = $props();

  const und = $derived(undecidedId(sc.columns));
  const colOf = (key: string) => sc.columns.find((c) => c.key === key)?.id ?? '';
  const seg = $derived(barSegments(sc.counts, sc.total));
  const total = $derived(sc.total > 0 ? sc.total : 513);
  const custom = $derived(sc.columns.filter((c) => !c.key));
  const tiles = $derived([
    { key: 'yes', label: `Sim (mínimo ${sc.threshold})`, n: sc.counts.yes, tone: 'yes' },
    { key: 'no', label: 'Não', n: sc.counts.no, tone: 'no' },
    { key: 'disputed', label: 'Em disputa', n: sc.counts.disputed, tone: 'disputed' },
    { key: 'undecided', label: 'Sem posição', n: sc.column_counts[und] ?? 0, tone: '' },
    { key: 'absent', label: 'Ausentes', n: sc.counts.absent, tone: '' },
    { key: 'chair', label: 'Presidente', n: sc.counts.chair, tone: '' },
  ]);
</script>

<section class="score" aria-label="Placar">
  <div class="tiles">
    {#each tiles as t (t.key)}
      <div class="tile {t.tone}">
        <span class="label">{t.label}</span>
        <b class="n">{t.n}</b>
        {#if priorities.length}
          <span class="prio" title="Prioritários nesta coluna">★ {prioritiesIn(colOf(t.key), priorities, sc.positions, und)}</span>
        {/if}
      </div>
    {/each}
  </div>
  <div
    class="bar"
    role="img"
    aria-label={`Sim ${sc.counts.yes}, Ausentes ${sc.counts.absent}, Presidente ${sc.counts.chair}, Não ${sc.counts.no}, de ${total}`}
  >
    <div class="s yes" style:width={`${seg.yes}%`}>{seg.yes > 4 ? sc.counts.yes : ''}</div>
    <div class="s gap"></div>
    <div class="s absent" style:width={`${seg.absent}%`}></div>
    <div class="s chair" style:width={`${seg.chair}%`}></div>
    <div class="s no" style:width={`${seg.no}%`}>{seg.no > 4 ? sc.counts.no : ''}</div>
    <div class="marker" style:left={`${(100 * sc.threshold) / total}%`} title={`Mínimo: ${sc.threshold}`}></div>
  </div>
  <p class="outcome {sc.outcome.state}">{outcomeText(sc)}</p>
  {#if custom.length}
    <p class="extra">Colunas próprias: {custom.map((c) => `${c.name} ${sc.column_counts[c.id] ?? 0}`).join(' · ')}</p>
  {/if}
  {#if orientationsText(sc.orientations)}
    <p class="extra">
      Orientações: {orientationsText(sc.orientations)}{sc.dissidents.length ? ` · ${sc.dissidents.length} dissidentes` : ''}
    </p>
  {/if}
</section>

<style>
  .score {
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    padding: var(--sp-4) var(--sp-5);
    margin-bottom: var(--sp-5);
  }
  .tiles {
    display: grid;
    gap: var(--sp-2);
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 8.5rem), 1fr));
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--sp-2) var(--sp-3);
    border-radius: var(--r-sm);
    background: var(--surface-2);
    border: 1px solid var(--border-subtle);
    min-width: 0;
  }
  .tile.yes {
    background: var(--accent-soft);
    border-color: color-mix(in srgb, var(--accent) 35%, transparent);
  }
  .tile.no {
    background: var(--danger-soft);
    border-color: color-mix(in srgb, var(--danger) 35%, transparent);
  }
  .tile.disputed {
    background: var(--warning-soft);
    border-color: color-mix(in srgb, var(--warning) 35%, transparent);
  }
  .label {
    font-size: var(--fs-xs);
    color: var(--text-2);
  }
  .n {
    font-size: var(--fs-2xl);
    line-height: 1.1;
    font-variant-numeric: tabular-nums;
  }
  .prio {
    font-size: var(--fs-xs);
    color: var(--text-3);
  }
  .bar {
    position: relative;
    display: flex;
    height: 1.75rem;
    margin-top: var(--sp-4);
    border-radius: var(--r-sm);
    background: var(--surface-3);
    overflow: hidden;
  }
  .s {
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: var(--fs-xs);
    font-weight: var(--fw-semibold);
    color: #fff;
    white-space: nowrap;
  }
  .s.yes {
    background: var(--accent);
  }
  .s.no {
    background: var(--danger);
  }
  .s.absent {
    background: var(--text-3);
  }
  .s.chair {
    background: var(--info);
  }
  .s.gap {
    flex: 1;
  }
  .marker {
    position: absolute;
    top: -2px;
    bottom: -2px;
    width: 2px;
    background: var(--text-1);
  }
  .outcome {
    margin: var(--sp-3) 0 0;
    font-weight: var(--fw-semibold);
  }
  .outcome.approved {
    color: var(--accent-strong);
  }
  .outcome.rejected {
    color: var(--danger);
  }
  .extra {
    margin: var(--sp-1) 0 0;
    font-size: var(--fs-sm);
    color: var(--text-3);
  }
</style>
