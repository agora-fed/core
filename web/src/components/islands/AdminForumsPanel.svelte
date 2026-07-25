<script lang="ts">
  // Painel admin dos fóruns (F3): curadoria do e-mail institucional, patamares
  // de envio e moderadores de cada fórum. Busca por caminho/nome; edição inline.
  import { onMount } from 'svelte';
  import {
    adminForumAddModerator,
    adminForumModerators,
    adminForumRemoveModerator,
    adminListForums,
    adminUpdateForum,
    type AdminForumDto,
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
            </td>
          </tr>
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
