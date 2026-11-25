<script lang="ts">
  // Columns of one scenario. Click selects (shift = range), double click or Enter opens the
  // deputy, drag moves the selection (or one card, or a whole party/UF section).
  import { byColumn, sections, type Column, type Deputy, type GroupBy, type Position } from '../../lib/articulacao';

  interface Props {
    columns: Column[];
    positions: Position[];
    columnCounts: Record<string, number>;
    deputies: Deputy[];
    priorities: ReadonlySet<number>;
    dissidents: ReadonlySet<number>;
    groupBy: GroupBy;
    selected: ReadonlySet<number>;
    canEdit: boolean;
    onselect: (id: number, order: number[], range: boolean) => void;
    onopen: (id: number) => void;
    onmove: (ids: number[], columnId: string) => void;
    onselectall: (ids: number[]) => void;
    oneditcolumn: (column: Column) => void;
  }
  let {
    columns,
    positions,
    columnCounts,
    deputies,
    priorities,
    dissidents,
    groupBy,
    selected,
    canEdit,
    onselect,
    onopen,
    onmove,
    onselectall,
    oneditcolumn,
  }: Props = $props();

  const map = $derived(byColumn(columns, positions, deputies));
  const order = $derived(deputies.map((d) => d.camara_id));
  let over = $state<string | null>(null);

  function drag(e: DragEvent, ids: number[]) {
    if (!e.dataTransfer) return;
    e.dataTransfer.setData('application/x-diogenes', JSON.stringify(ids));
    e.dataTransfer.effectAllowed = 'move';
  }
  function drop(e: DragEvent, columnId: string) {
    e.preventDefault();
    over = null;
    try {
      const ids = JSON.parse(e.dataTransfer?.getData('application/x-diogenes') ?? '[]');
      if (Array.isArray(ids) && ids.length && ids.every((x) => Number.isInteger(x))) onmove(ids, columnId);
    } catch {
      /* not ours */
    }
  }
  const idsFor = (d: Deputy) => (selected.has(d.camara_id) ? [...selected] : [d.camara_id]);
</script>

<div class="board" role="list">
  {#each columns as col (col.id)}
    {@const items = map.get(col.id) ?? []}
    {@const total = columnCounts[col.id] ?? 0}
    <section
      class="col"
      class:over={over === col.id}
      aria-label={col.name}
      role="listitem"
      ondragover={(e) => {
        if (!canEdit) return;
        e.preventDefault();
        over = col.id;
      }}
      ondragleave={() => (over = over === col.id ? null : over)}
      ondrop={(e) => canEdit && drop(e, col.id)}
    >
      <header class="col-head">
        <span class="dot" style:background={col.color}></span>
        <b class="col-name">{col.name}</b>
        <span class="count" title="Total no cenário">{total}{items.length !== total ? ` · ${items.length} no filtro` : ''}</span>
        {#if canEdit}
          <span class="col-actions">
            <button type="button" class="link" onclick={() => onselectall(items.map((d) => d.camara_id))} disabled={!items.length}>
              Selecionar
            </button>
            {#if !col.key}
              <button type="button" class="link" aria-label={`Editar coluna ${col.name}`} onclick={() => oneditcolumn(col)}>Editar</button>
            {/if}
          </span>
        {/if}
      </header>
      <div class="col-body">
        {#each sections(items, groupBy) as s (s.key)}
          {#if s.key}
            <div
              class="section-head"
              draggable={canEdit}
              ondragstart={(e) => drag(e, s.items.map((d) => d.camara_id))}
              role="presentation"
            >
              {s.key} · {s.items.length}
            </div>
          {/if}
          {#each s.items as d (d.camara_id)}
            {@const isSel = selected.has(d.camara_id)}
            <button
              type="button"
              class="dep"
              class:sel={isSel}
              aria-pressed={canEdit ? isSel : undefined}
              draggable={canEdit}
              title={canEdit ? 'Clique para selecionar; duplo clique abre a ficha' : 'Abrir a ficha'}
              ondragstart={(e) => drag(e, idsFor(d))}
              onclick={(e) => (canEdit ? onselect(d.camara_id, order, e.shiftKey) : onopen(d.camara_id))}
              ondblclick={() => onopen(d.camara_id)}
              onkeydown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  onopen(d.camara_id);
                }
              }}
            >
              {#if priorities.has(d.camara_id)}<span class="star" title="Prioritário">★</span>{/if}
              <span class="name">{d.name}</span>
              {#if d.board_role === 'Presidente'}<span class="tag">PRES</span>{/if}
              {#if d.leader_role}<span class="tag" title={d.leader_role}>LÍD</span>{/if}
              {#if dissidents.has(d.camara_id)}<span class="tag diss" title="Votou contra a orientação">DISS</span>{/if}
              <span class="meta">{d.party}/{d.uf}</span>
            </button>
          {/each}
        {/each}
        {#if items.length === 0}<p class="empty">Ninguém aqui.</p>{/if}
      </div>
    </section>
  {/each}
</div>

<style>
  .board {
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: minmax(15rem, 1fr);
    gap: var(--sp-3);
    overflow-x: auto;
    padding-bottom: var(--sp-2);
    align-items: start;
  }
  .col {
    display: flex;
    flex-direction: column;
    min-width: 0;
    max-height: 70vh;
    background: var(--surface-2);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    transition: border-color var(--dur-fast) var(--ease-out);
  }
  .col.over {
    border-color: var(--accent);
    box-shadow: var(--shadow-focus);
  }
  .col-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-3);
    border-bottom: 1px solid var(--border-subtle);
  }
  .dot {
    width: 0.65rem;
    height: 0.65rem;
    border-radius: var(--r-full);
    flex: none;
  }
  .col-name {
    font-size: var(--fs-sm);
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .count {
    font-size: var(--fs-xs);
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
  .col-actions {
    margin-left: auto;
    display: inline-flex;
    gap: var(--sp-2);
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    font: inherit;
    font-size: var(--fs-xs);
    color: var(--accent-strong);
    cursor: pointer;
  }
  .link:disabled {
    color: var(--text-3);
    cursor: default;
  }
  .col-body {
    overflow-y: auto;
    padding: var(--sp-2);
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
  }
  .section-head {
    margin-top: var(--sp-2);
    font-size: var(--fs-xs);
    font-weight: var(--fw-semibold);
    color: var(--text-2);
    cursor: grab;
  }
  .dep {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    width: 100%;
    padding: var(--sp-2);
    text-align: left;
    font: inherit;
    font-size: var(--fs-sm);
    color: var(--text-1);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    cursor: pointer;
  }
  .dep[draggable='true'] {
    cursor: grab;
  }
  .dep:hover {
    border-color: var(--border-strong);
  }
  .dep:focus-visible {
    outline: none;
    box-shadow: var(--shadow-focus);
  }
  .dep.sel {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .name {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    font-size: var(--fs-xs);
    color: var(--text-3);
    white-space: nowrap;
  }
  .star {
    color: var(--warning);
  }
  .tag {
    font-size: 0.65rem;
    font-weight: var(--fw-bold);
    padding: 0 4px;
    border-radius: var(--r-xs);
    background: var(--surface-3);
    color: var(--text-2);
  }
  .tag.diss {
    background: var(--warning-soft);
    color: var(--warning);
  }
  .empty {
    margin: var(--sp-2);
    font-size: var(--fs-xs);
    color: var(--text-3);
  }
</style>
