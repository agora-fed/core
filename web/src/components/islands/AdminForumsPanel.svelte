<script lang="ts">
  // Painel admin dos fóruns (F3): curadoria do e-mail institucional, patamares
  // de envio e moderadores de cada fórum. Busca por caminho/nome; edição inline.
  import { onMount } from 'svelte';
  import {
    adminArchiveForum,
    adminCreateForum,
    adminForumAddMember,
    adminForumAddModerator,
    adminForumMembers,
    adminForumRemoveMember,
    adminForumModerators,
    adminForumRemoveModerator,
    adminListForums,
    adminUpdateForum,
    getAdminUsers,
    type AdminForumDto,
    type AdminUserRow,
    type ForumVisibility,
    type ForumModeratorDto,
  } from '../../lib/api';

  let rows = $state<AdminForumDto[]>([]);
  let q = $state('');
  let loading = $state(true);
  let msg = $state<string | null>(null);

  // Edição inline: por id do fórum.
  let emailDraft = $state<Record<string, string>>({});
  let thrDraft = $state<Record<string, string>>({});
  let openMods = $state<string | null>(null);
  let mods = $state<ForumModeratorDto[]>([]);
  let newMod = $state('');
  let busy = $state(false);
  let accessDraft = $state<Record<string, ForumVisibility>>({});
  let federatedDraft = $state<Record<string, boolean>>({});
  let openMembers = $state<string | null>(null);
  let members = $state<ForumModeratorDto[]>([]);
  let personQuery = $state('');
  let personResults = $state<AdminUserRow[]>([]);
  let newForum = $state({ slug: '', name: '', description: '', visibility: 'public' as ForumVisibility, federated: false });

  const ACCESS_LABEL: Record<ForumVisibility, string> = {
    public: 'Público (qualquer pessoa, mesmo sem login)',
    members: 'Membros (qualquer conta com login)',
    list: 'Lista (só as pessoas adicionadas)',
  };

  async function createForum() {
    busy = true;
    const res = await adminCreateForum({
      ...newForum,
      federated: newForum.visibility === 'public' && newForum.federated,
    });
    busy = false;
    if (!res.success) {
      msg = res.error?.message ?? 'Não foi possível criar o fórum.';
      return;
    }
    newForum = { slug: '', name: '', description: '', visibility: 'public', federated: false };
    await load();
  }

  async function archiveForum(r: AdminForumDto) {
    if (!confirm(`Arquivar /f/${r.full_path}? O fórum e seus tópicos deixam de aparecer.`)) return;
    busy = true;
    const res = await adminArchiveForum(r.id);
    busy = false;
    if (!res.success) {
      msg = res.error?.message ?? 'Não foi possível arquivar.';
      return;
    }
    await load();
  }

  async function saveAccess(r: AdminForumDto) {
    const visibility = accessDraft[r.id];
    if (
      visibility !== 'public' && r.federated &&
      !confirm('Tornar restrito para a federação deste fórum. O que já foi enviado a outros servidores não é recolhido. Continuar?')
    ) return;
    busy = true;
    const res = await adminUpdateForum(r.id, {
      visibility,
      federated: visibility === 'public' ? federatedDraft[r.id] : false,
    });
    busy = false;
    if (!res.success) {
      msg = res.error?.message ?? 'Não foi possível salvar o acesso.';
      return;
    }
    await load();
  }

  async function toggleMembers(r: AdminForumDto) {
    if (openMembers === r.id) {
      openMembers = null;
      return;
    }
    openMembers = r.id;
    members = [];
    personQuery = '';
    personResults = [];
    const res = await adminForumMembers(r.id);
    if (res.success && res.data) members = res.data;
  }

  async function searchPeople() {
    const res = await getAdminUsers(personQuery.trim(), 10);
    personResults = res.success && res.data ? res.data : [];
  }

  async function addMember(r: AdminForumDto, person: AdminUserRow) {
    busy = true;
    const res = await adminForumAddMember(r.id, person.citizen_id);
    busy = false;
    if (!res.success) {
      msg = res.error?.message ?? 'Não foi possível adicionar.';
      return;
    }
    personResults = [];
    personQuery = '';
    const list = await adminForumMembers(r.id);
    if (list.success && list.data) members = list.data;
    await load();
    openMembers = r.id;
  }

  async function removeMember(r: AdminForumDto, citizenId: string) {
    const res = await adminForumRemoveMember(r.id, citizenId);
    if (res.success) members = members.filter((m) => m.citizen_id !== citizenId);
  }

  async function load() {
    loading = true;
    msg = null;
    const res = await adminListForums(q.trim());
    loading = false;
    if (res.success && res.data) {
      rows = res.data;
      for (const r of rows) {
        emailDraft[r.id] = r.contact_email ?? '';
        thrDraft[r.id] = r.thresholds.join(', ');
        accessDraft[r.id] = r.visibility;
        federatedDraft[r.id] = r.federated;
      }
    } else {
      msg = res.error?.message ?? 'Não foi possível carregar (é admin?).';
    }
  }

  onMount(() => void load());

  /** "1000, 10000 100000" → [1000, 10000, 100000]; null se inválido/não-crescente. */
  function parseThresholds(s: string): number[] | null {
    const parts = s
      .split(/[,\s]+/)
      .filter(Boolean)
      .map((x) => Number(x));
    if (parts.length === 0 || parts.some((n) => !Number.isInteger(n) || n <= 0)) {
      return null;
    }
    for (let i = 1; i < parts.length; i++) {
      if (parts[i] <= parts[i - 1]) return null;
    }
    return parts;
  }

  async function save(r: AdminForumDto) {
    if (busy) return;
    const ts = parseThresholds(thrDraft[r.id] ?? '');
    if (!ts) {
      msg = `Patamares inválidos em ${r.full_path} — use inteiros crescentes (ex.: 1000, 10000, 100000).`;
      return;
    }
    busy = true;
    const res = await adminUpdateForum(r.id, {
      contact_email: (emailDraft[r.id] ?? '').trim(),
      thresholds: ts,
    });
    busy = false;
    if (res.success) {
      msg = `✅ ${r.full_path} salvo. Patamares pendentes disparam no próximo ciclo do carteiro.`;
      void load();
    } else {
      msg = res.error?.message ?? 'Falha ao salvar.';
    }
  }

  async function toggleMods(r: AdminForumDto) {
    if (openMods === r.id) {
      openMods = null;
      return;
    }
    openMods = r.id;
    mods = [];
    const res = await adminForumModerators(r.id);
    if (res.success && res.data) mods = res.data;
  }

  async function addMod(r: AdminForumDto) {
    if (busy || !newMod.trim()) return;
    busy = true;
    const res = await adminForumAddModerator(r.id, newMod.trim());
    busy = false;
    if (res.success) {
      newMod = '';
      const list = await adminForumModerators(r.id);
      if (list.success && list.data) mods = list.data;
      void load();
    } else {
      msg = res.error?.message ?? 'Falha ao adicionar moderador.';
    }
  }

  async function delMod(r: AdminForumDto, citizenId: string) {
    if (busy) return;
    busy = true;
    const ok = await adminForumRemoveModerator(r.id, citizenId);
    busy = false;
    if (ok) mods = mods.filter((m) => m.citizen_id !== citizenId);
  }
</script>

<div class="panel">
  <form class="bar" onsubmit={(e) => { e.preventDefault(); void load(); }}>
    <input class="input" type="search" bind:value={q} placeholder="Buscar por caminho ou nome (ex.: senado/ccj, saude, sp/santos)…" />
    <button class="btn" type="submit">Buscar</button>
  </form>
  {#if msg}<p class="note" role="status">{msg}</p>{/if}

  <details class="new-forum">
    <summary>Criar fórum</summary>
    <form onsubmit={(e) => { e.preventDefault(); void createForum(); }}>
      <label>Nome <input class="input" type="text" maxlength="120" bind:value={newForum.name} required /></label>
      <label>Endereço (/f/…) <input class="input" type="text" maxlength="79" pattern="[a-z0-9][a-z0-9\-]+" bind:value={newForum.slug} required /></label>
      <label>Descrição <input class="input" type="text" bind:value={newForum.description} /></label>
      <label>Acesso
        <select class="input" bind:value={newForum.visibility}>
          {#each Object.entries(ACCESS_LABEL) as [value, label] (value)}
            <option {value}>{label}</option>
          {/each}
        </select>
      </label>
      {#if newForum.visibility === 'public'}
        <label class="check"><input type="checkbox" bind:checked={newForum.federated} /> Publicar também no fediverso</label>
      {/if}
      <button class="btn" type="submit" disabled={busy}>Criar</button>
    </form>
  </details>

  {#if loading}
    <p class="muted">Carregando…</p>
  {:else}
    <table>
      <thead>
        <tr>
          <th>Fórum</th>
          <th>E-mail institucional</th>
          <th>Patamares</th>
          <th>Tópicos</th>
          <th>Pend.</th>
          <th></th>
        </tr>
      </thead>
      <tbody>
        {#each rows as r (r.id)}
          <tr>
            <td>
              <a href={`/f/${r.full_path}`} target="_blank" rel="noopener">/f/{r.full_path}</a>
              <div class="muted small">{r.name}</div>
            </td>
            <td>
              <input class="input" type="email" bind:value={emailDraft[r.id]} placeholder="(em curadoria)" />
            </td>
            <td>
              <input class="input thr" type="text" bind:value={thrDraft[r.id]} />
            </td>
            <td class="num">{r.topic_count}</td>
            <td class="num" title="Envios pendentes de e-mail">{r.pending_dispatches > 0 ? `📨 ${r.pending_dispatches}` : '—'}</td>
            <td class="actions">
              <button class="btn" type="button" onclick={() => save(r)} disabled={busy}>Salvar</button>
              <button class="btn" type="button" onclick={() => toggleMods(r)}>
                Mods ({r.moderator_count})
              </button>
              <button class="btn" type="button" onclick={() => toggleMembers(r)}>
                Acesso ({r.visibility === 'list' ? `${r.member_count} na lista` : r.visibility === 'members' ? 'membros' : 'público'})
              </button>
              <button class="btn" type="button" onclick={() => archiveForum(r)} disabled={busy}>Arquivar</button>
            </td>
          </tr>
          {#if openMembers === r.id}
            <tr class="mods-row">
              <td colspan="6">
                <div class="mod-add">
                  <select class="input" bind:value={accessDraft[r.id]}>
                    {#each Object.entries(ACCESS_LABEL) as [value, label] (value)}
                      <option {value}>{label}</option>
                    {/each}
                  </select>
                  {#if accessDraft[r.id] === 'public'}
                    <label class="check"><input type="checkbox" bind:checked={federatedDraft[r.id]} /> Publicar no fediverso</label>
                  {/if}
                  <button class="btn" type="button" onclick={() => saveAccess(r)} disabled={busy}>Salvar acesso</button>
                </div>
                {#if accessDraft[r.id] === 'members'}
                  <p class="muted small">Qualquer conta com login lê este fórum. Para assuntos sensíveis, use a lista.</p>
                {/if}
                {#if r.visibility === 'list'}
                  {#if members.length === 0}
                    <p class="muted">Ninguém na lista ainda.</p>
                  {:else}
                    {#each members as m (m.citizen_id)}
                      <span class="mod-chip">
                        {m.display_name ?? `@${m.handle ?? '—'}`}
                        <button type="button" title="Remover" onclick={() => removeMember(r, m.citizen_id)}>✕</button>
                      </span>
                    {/each}
                  {/if}
                  <form class="mod-add" onsubmit={(e) => { e.preventDefault(); void searchPeople(); }}>
                    <input class="input" type="text" bind:value={personQuery} placeholder="Buscar por nome, @ ou e-mail" />
                    <button class="btn" type="submit" disabled={!personQuery.trim()}>Buscar</button>
                  </form>
                  {#each personResults as p (p.citizen_id)}
                    <div class="mod-add">
                      <span>{p.display_name ?? '—'} {p.handle ? `@${p.handle}` : ''}</span>
                      <button class="btn" type="button" onclick={() => addMember(r, p)} disabled={busy}>Adicionar</button>
                    </div>
                  {/each}
                {/if}
              </td>
            </tr>
          {/if}
          {#if openMods === r.id}
            <tr class="mods-row">
              <td colspan="6">
                {#if mods.length === 0}
                  <span class="muted">Nenhum moderador designado.</span>
                {:else}
                  {#each mods as m (m.citizen_id)}
                    <span class="mod-chip">
                      @{m.handle ?? m.citizen_id.slice(0, 8)}
                      <button type="button" title="Remover" onclick={() => delMod(r, m.citizen_id)}>✕</button>
                    </span>
                  {/each}
                {/if}
                <form class="mod-add" onsubmit={(e) => { e.preventDefault(); void addMod(r); }}>
                  <input class="input" type="text" bind:value={newMod} placeholder="@handle do cidadão" />
                  <button class="btn" type="submit" disabled={busy || !newMod.trim()}>Adicionar</button>
                </form>
              </td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
    <p class="muted small">
      Fórum sem e-mail: patamares cruzados ficam <strong>pendentes</strong> e o
      carteiro dispara retroativamente assim que o e-mail for salvo. Sub-fóruns sem
      e-mail próprio herdam o do pai.
    </p>
  {/if}
</div>

<style>
  .panel { display: block; }
  .bar { display: flex; gap: 0.5rem; margin-bottom: 0.75rem; }
  .bar .input { flex: 1; }
  table { width: 100%; border-collapse: collapse; font-size: 0.92rem; }
  th, td { text-align: left; padding: 0.4rem 0.5rem; border-bottom: 1px solid var(--c-border, #333); vertical-align: top; }
  .num { text-align: right; white-space: nowrap; }
  .thr { min-width: 11rem; }
  .actions { white-space: nowrap; display: flex; gap: 0.3rem; }
  .actions { display: flex; flex-wrap: wrap; gap: 0.25rem; max-width: 14rem; }
  .panel { overflow-x: auto; }
  .new-forum { margin: 0.75rem 0; }
  .new-forum form { display: grid; gap: 0.5rem; max-width: 32rem; margin-top: 0.5rem; }
  .new-forum label { display: grid; gap: 0.2rem; }
  .check { display: flex !important; align-items: center; gap: 0.4rem; }
  .mods-row td { background: rgba(127, 127, 127, 0.06); }
  .mod-chip {
    display: inline-flex; align-items: center; gap: 0.3rem;
    border: 1px solid var(--c-border, #555); border-radius: 999px;
    padding: 0.1rem 0.6rem; margin: 0 0.3rem 0.3rem 0; font-size: 0.88rem;
  }
  .mod-chip button { background: none; border: none; cursor: pointer; color: inherit; }
  .mod-add { display: flex; gap: 0.4rem; margin-top: 0.4rem; max-width: 24rem; }
  .note { margin: 0.5rem 0; }
  .small { font-size: 0.85rem; }
</style>
