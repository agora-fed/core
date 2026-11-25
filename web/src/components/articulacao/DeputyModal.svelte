<script lang="ts">
  // The deputy card: position, priority, notes of the room, groups.
  import Modal from '../ui/Modal.svelte';
  import Button from '../ui/Button.svelte';
  import Select from '../ui/Select.svelte';
  import Textarea from '../ui/Textarea.svelte';
  import Avatar from '../ui/Avatar.svelte';
  import {
    camaraUrl,
    fmtTime,
    href,
    safeUrl,
    type Column,
    type Deputy,
    type Group,
    type Note,
  } from '../../lib/articulacao';

  interface Props {
    open: boolean;
    deputy: Deputy;
    groups: Group[];
    columns: Column[];
    columnId: string;
    isPriority: boolean;
    notes: Note[];
    canEdit: boolean;
    onclose: () => void;
    onmove: (columnId: string) => void;
    onpriority: (value: boolean) => void;
    onaddnote: (body: string, isPrivate: boolean) => Promise<boolean>;
    ondeletenote: (id: string) => void;
  }
  let { open = $bindable(), deputy: d, groups, columns, columnId, isPriority, notes, canEdit, onclose, onmove, onpriority, onaddnote, ondeletenote }: Props = $props();

  let body = $state('');
  let isPrivate = $state(false);
  let busy = $state(false);
  let position = $state('');
  $effect(() => {
    position = columnId;
  });

  async function save(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    const ok = await onaddnote(body, isPrivate);
    busy = false;
    if (ok) {
      body = '';
      isPrivate = false;
    }
  }
</script>

<Modal bind:open title={d.name} size="lg" {onclose}>
  <div class="who">
    <Avatar src={safeUrl(d.photo_url)} name={d.name} size="lg" />
    <div>
      <p class="line strong">{d.party} / {d.uf}</p>
      {#if d.board_role}<p class="line">Mesa: {d.board_role}</p>{/if}
      {#if d.leader_role}<p class="line">Liderança: {d.leader_role}</p>{/if}
      <a href={camaraUrl(d.camara_id)} target="_blank" rel="noopener noreferrer">Página na Câmara ↗</a>
    </div>
  </div>

  {#if canEdit}
    <div class="controls">
      <Select
        label="Posição neste cenário"
        bind:value={position}
        options={columns.map((c) => ({ value: c.id, label: c.name }))}
        onchange={(v) => onmove(v)}
      />
      <label class="art-check">
        <input type="checkbox" checked={isPriority} onchange={(e) => onpriority(e.currentTarget.checked)} />
        Prioritário neste projeto
      </label>
    </div>
  {/if}

  <h3 class="h3">Anotações</h3>
  {#if notes.length === 0}
    <p class="muted small">Nenhuma anotação.</p>
  {:else}
    <ul class="notes">
      {#each notes as n (n.id)}
        <li class="note" class:private={n.visibility === 'author'}>
          <p class="meta">
            {n.author_name ?? 'ex-membro'} · {fmtTime(n.updated_at)}{n.visibility === 'author' ? ' · só você vê' : ''}
          </p>
          <p class="text">{n.body}</p>
          {#if canEdit}
            <Button variant="ghost" size="sm" onclick={() => ondeletenote(n.id)}>Excluir</Button>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
  {#if canEdit}
    <form onsubmit={save}>
      <Textarea label="Nova anotação" bind:value={body} rows={3} maxlength={10000} required />
      <div class="row">
        <label class="art-check"><input type="checkbox" bind:checked={isPrivate} /> Só eu posso ver</label>
        <Button type="submit" variant="primary" size="sm" loading={busy} disabled={!body.trim()}>Salvar anotação</Button>
      </div>
    </form>
  {/if}

  <details class="groups">
    <summary>Frentes e bancadas ({groups.length})</summary>
    {#if groups.length}
      <ul>
        {#each groups as g (g.id)}<li><a href={href.group(g.id)} onclick={onclose}>{g.title}</a></li>{/each}
      </ul>
    {:else}
      <p class="muted small">Nenhuma registrada.</p>
    {/if}
  </details>
  <p class="muted small">Anotações são sobre a atuação pública do parlamentar. Não registre dados pessoais sensíveis.</p>
</Modal>

<style>
  .who {
    display: flex;
    gap: var(--sp-4);
    align-items: center;
    margin-bottom: var(--sp-4);
  }
  .line {
    margin: 0;
    font-size: var(--fs-sm);
  }
  .strong {
    font-weight: var(--fw-semibold);
  }
  .controls {
    display: grid;
    gap: var(--sp-2);
    margin-bottom: var(--sp-4);
  }
  .h3 {
    font-size: var(--fs-base);
    margin: var(--sp-4) 0 var(--sp-2);
  }
  .small {
    font-size: var(--fs-sm);
  }
  .notes {
    list-style: none;
    padding: 0;
    margin: 0 0 var(--sp-3);
    display: grid;
    gap: var(--sp-2);
  }
  .note {
    padding: var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-2);
  }
  .note.private {
    border-style: dashed;
  }
  .meta {
    margin: 0 0 var(--sp-1);
    font-size: var(--fs-xs);
    color: var(--text-3);
  }
  .text {
    margin: 0 0 var(--sp-1);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    align-items: center;
    gap: var(--sp-2);
  }
  .groups {
    margin: var(--sp-4) 0 var(--sp-2);
  }
  .groups summary {
    cursor: pointer;
    font-weight: var(--fw-semibold);
    font-size: var(--fs-sm);
  }
</style>
