<script lang="ts">
  // Painel admin: CRUD das áreas de interesse (interest_area, migration 0661). Adicionar,
  // editar inline, reordenar (position) e remover — só remove se nenhum cidadão usar a área.
  import { onMount } from 'svelte';
  import {
    getAdminInterestAreas,
    createInterestArea,
    updateInterestArea,
    deleteInterestArea,
    type AdminInterestArea,
  } from '../../lib/api';

  let items = $state<AdminInterestArea[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);

  // Edição inline: slug da linha em edição + rascunho dos campos.
  let editingSlug = $state<string | null>(null);
  let draftName = $state('');
  let draftMinistry = $state('');
  let draftPosition = $state(0);
  let saving = $state(false);

  // Formulário de criação.
  let newSlug = $state('');
  let newName = $state('');
  let newMinistry = $state('');
  let newPosition = $state(0);
  let creating = $state(false);

  async function load() {
    loading = true;
    error = null;
    const res = await getAdminInterestAreas();
    loading = false;
    if (res.success && res.data) {
      items = res.data;
    } else {
      error = res.error?.message ?? 'Não foi possível carregar as áreas.';
    }
  }

  function flash(msg: string) {
    notice = msg;
    setTimeout(() => (notice = null), 3000);
  }

  function startEdit(a: AdminInterestArea) {
    editingSlug = a.slug;
    draftName = a.name;
    draftMinistry = a.ministry ?? '';
    draftPosition = a.position;
  }
  function cancelEdit() {
    editingSlug = null;
  }

  async function saveEdit(slug: string) {
    if (!draftName.trim()) {
      error = 'Nome é obrigatório.';
      return;
    }
    saving = true;
    error = null;
    const res = await updateInterestArea(slug, {
      name: draftName.trim(),
      ministry: draftMinistry.trim() || null,
      position: draftPosition,
    });
    saving = false;
    if (res.success) {
      editingSlug = null;
      flash('Área atualizada.');
      await load();
    } else {
      error = res.error?.message ?? 'Não foi possível salvar.';
    }
  }

  async function create() {
    if (!newSlug.trim() || !newName.trim()) {
      error = 'Slug e nome são obrigatórios.';
      return;
    }
    creating = true;
    error = null;
    const res = await createInterestArea({
      slug: newSlug.trim(),
      name: newName.trim(),
      ministry: newMinistry.trim() || null,
      position: newPosition,
    });
    creating = false;
    if (res.success) {
      newSlug = '';
      newName = '';
      newMinistry = '';
      newPosition = 0;
      flash('Área criada.');
      await load();
    } else {
      error = res.error?.message ?? 'Não foi possível criar a área.';
    }
  }

  async function remove(a: AdminInterestArea) {
    if (a.citizen_count > 0) {
      error = `"${a.name}" está em uso por ${a.citizen_count} cidadão(s) e não pode ser removida.`;
      return;
    }
    if (!confirm(`Remover a área "${a.name}"?`)) return;
    error = null;
    const res = await deleteInterestArea(a.slug);
    if (res.success) {
      flash('Área removida.');
      await load();
    } else {
      error = res.error?.message ?? 'Não foi possível remover.';
    }
  }

  onMount(load);
</script>

{#if notice}
  <div class="card ok" role="status">{notice}</div>
{/if}
{#if error}
  <div class="card err" role="alert">{error}</div>
{/if}

<div class="create card">
  <h2>Nova área</h2>
  <div class="fields">
    <label>
      <span>Slug</span>
      <input type="text" bind:value={newSlug} placeholder="ex.: saude" />
    </label>
    <label>
      <span>Nome</span>
      <input type="text" bind:value={newName} placeholder="ex.: Saúde" />
    </label>
    <label>
      <span>Ministério</span>
      <input type="text" bind:value={newMinistry} placeholder="opcional" />
    </label>
    <label class="pos">
      <span>Ordem</span>
      <input type="number" bind:value={newPosition} />
    </label>
    <button class="btn" onclick={create} disabled={creating}>
      {creating ? 'Criando…' : 'Adicionar'}
    </button>
  </div>
</div>

<div class="table-wrap">
  <table>
    <thead>
      <tr><th>Ordem</th><th>Slug</th><th>Nome</th><th>Ministério</th><th>Cidadãos</th><th>Ações</th></tr>
    </thead>
    <tbody>
      {#if loading}
        <tr><td colspan="6" class="muted center">Carregando…</td></tr>
      {:else if items.length === 0}
        <tr><td colspan="6" class="muted center">Nenhuma área cadastrada.</td></tr>
      {:else}
        {#each items as a (a.slug)}
          <tr>
            {#if editingSlug === a.slug}
              <td><input type="number" class="inp-pos" bind:value={draftPosition} /></td>
              <td class="mono small">{a.slug}</td>
              <td><input type="text" bind:value={draftName} /></td>
              <td><input type="text" bind:value={draftMinistry} placeholder="—" /></td>
              <td class="center small">{a.citizen_count}</td>
              <td class="actions">
                <button class="btn small" onclick={() => saveEdit(a.slug)} disabled={saving}>
                  {saving ? 'Salvando…' : 'Salvar'}
                </button>
                <button class="btn ghost small" onclick={cancelEdit} disabled={saving}>Cancelar</button>
              </td>
            {:else}
              <td class="center small">{a.position}</td>
              <td class="mono small">{a.slug}</td>
              <td>{a.name}</td>
              <td class="small">{a.ministry ?? '—'}</td>
              <td class="center small">{a.citizen_count.toLocaleString('pt-BR')}</td>
              <td class="actions">
                <button class="btn ghost small" onclick={() => startEdit(a)}>Editar</button>
                <button
                  class="btn danger small"
                  onclick={() => remove(a)}
                  disabled={a.citizen_count > 0}
                  title={a.citizen_count > 0 ? 'Em uso; não removível' : 'Remover'}
                >Remover</button>
              </td>
            {/if}
          </tr>
        {/each}
      {/if}
    </tbody>
  </table>
</div>

<style>
  .card { background: var(--surface-1, #fff); border: 1px solid var(--border-subtle, rgba(0,0,0,0.1)); border-radius: 12px; padding: 1rem 1.2rem; margin-bottom: 1rem; }
  .ok { color: #15803d; border-color: #15803d; }
  .err { color: #dc2626; border-color: #dc2626; }
  .create h2 { margin: 0 0 0.7rem; font-size: 1rem; }
  .fields { display: flex; gap: 0.6rem; flex-wrap: wrap; align-items: end; }
  label { display: grid; gap: 0.2rem; font-size: 0.8rem; color: var(--muted, #64748b); }
  label.pos { max-width: 6rem; }
  input { padding: 0.5rem 0.6rem; border-radius: 8px; border: 1px solid var(--border-subtle, #cbd5e1); background: var(--surface-1, #fff); color: inherit; font: inherit; }
  .inp-pos { max-width: 4.5rem; }
  .btn { padding: 0.5rem 1rem; border-radius: 8px; border: 1px solid var(--c-ink, #0f172a); background: var(--surface-1, #fff); color: inherit; font-weight: 600; cursor: pointer; }
  .btn.ghost { border-color: var(--border-subtle, #cbd5e1); }
  .btn.danger { border-color: #dc2626; color: #dc2626; }
  .btn.small { padding: 0.35rem 0.7rem; font-size: 0.8rem; }
  .btn:disabled { opacity: 0.5; cursor: not-allowed; }
  .table-wrap { overflow-x: auto; border: 1px solid var(--border-subtle, rgba(0,0,0,0.1)); border-radius: 12px; }
  table { width: 100%; border-collapse: collapse; font-size: 0.92rem; }
  th, td { text-align: left; padding: 0.55rem 0.8rem; border-bottom: 1px solid var(--border-subtle, rgba(0,0,0,0.06)); vertical-align: middle; }
  th { font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.03em; color: var(--muted, #64748b); }
  td input { width: 100%; }
  .small { font-size: 0.82rem; }
  .center { text-align: center; }
  .mono { font-family: ui-monospace, monospace; }
  .muted { color: var(--muted, #64748b); }
  .actions { display: flex; gap: 0.4rem; white-space: nowrap; }
</style>
