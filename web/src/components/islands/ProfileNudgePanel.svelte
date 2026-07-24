<script lang="ts">
  // Painel do admin: convidar cidadãos com perfil incompleto a completar (0.49.0).
  // Só leitura/ação do admin — o envio é sempre por clique humano (nada automático).
  import { onMount } from 'svelte';
  import {
    getProfileNudgeOverview,
    getProfileNudgeCandidates,
    sendProfileNudge,
    type ProfileNudgeOverview,
    type ProfileNudgeCandidate,
  } from '../../lib/api';
  import { formatDate } from '../../lib/format';

  let loading = $state(true);
  let error = $state<string | null>(null);
  let overview = $state<ProfileNudgeOverview | null>(null);
  let candidates = $state<ProfileNudgeCandidate[]>([]);
  let selected = $state<Set<string>>(new Set());
  let busy = $state(false);
  let result = $state<string | null>(null);

  const selectedCount = $derived(selected.size);

  function toggle(id: string) {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    selected = next;
  }

  function selectNotYetNudged() {
    selected = new Set(candidates.filter((c) => !c.nudged_at).map((c) => c.citizen_id));
  }

  function clearSel() {
    selected = new Set();
  }

  async function load() {
    error = null;
    const [ov, cs] = await Promise.all([
      getProfileNudgeOverview(),
      getProfileNudgeCandidates(500),
    ]);
    if (ov.success && ov.data) overview = ov.data;
    if (cs.success && cs.data) candidates = cs.data;
    else if (!cs.success) error = cs.error?.message ?? 'Não foi possível carregar.';
  }

  async function convidar() {
    if (busy || selected.size === 0) return;
    const ids = [...selected].slice(0, 50);
    if (
      !confirm(
        `Enviar o convite pra completar o perfil a ${ids.length} cidadão(s)? Um e-mail real será enviado a cada um.`,
      )
    )
      return;
    busy = true;
    result = null;
    const res = await sendProfileNudge(ids);
    busy = false;
    if (res.success && res.data) {
      result = `Enviados: ${res.data.sent} · Pulados: ${res.data.skipped} · Falhas: ${res.data.failed}`;
      clearSel();
      await load();
    } else {
      result = res.error?.message ?? 'Não foi possível enviar.';
    }
  }

  onMount(async () => {
    await load();
    loading = false;
  });
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if error}
  <div class="card err" role="alert">{error}</div>
{:else}
  {#if overview}
    <div class="funnel">
      <div class="stat"><span class="n">{overview.total.toLocaleString('pt-BR')}</span><span class="muted">cidadãos</span></div>
      <div class="stat"><span class="n">{overview.incomplete.toLocaleString('pt-BR')}</span><span class="muted">perfil incompleto</span></div>
      <div class="stat"><span class="n hot">{overview.incomplete_not_nudged.toLocaleString('pt-BR')}</span><span class="muted">ainda não convidados</span></div>
    </div>
  {/if}

  <div class="toolbar">
    <button class="btn" onclick={selectNotYetNudged}>Selecionar não-convidados</button>
    <button class="btn ghost" onclick={clearSel} disabled={selectedCount === 0}>Limpar</button>
    <button class="btn primary" onclick={convidar} disabled={busy || selectedCount === 0}>
      {busy ? 'Enviando…' : `Convidar selecionados (${selectedCount})`}
    </button>
    {#if selectedCount > 50}<span class="warn">Só os 50 primeiros serão enviados por vez.</span>{/if}
  </div>
  {#if result}<p class="result">{result}</p>{/if}

  {#if candidates.length === 0}
    <div class="card"><p class="muted">Nenhum cidadão com perfil incompleto. 🎉</p></div>
  {:else}
    <div class="table-wrap">
      <table>
        <thead>
          <tr><th></th><th>E-mail</th><th>Nome</th><th>@usuário</th><th>Cadastro</th><th>Convite</th></tr>
        </thead>
        <tbody>
          {#each candidates as c (c.citizen_id)}
            <tr class:nudged={!!c.nudged_at}>
              <td><input type="checkbox" checked={selected.has(c.citizen_id)} onchange={() => toggle(c.citizen_id)} aria-label={`Selecionar ${c.email}`} /></td>
              <td class="email">{c.email}</td>
              <td>{c.display_name ?? '—'}</td>
              <td>{c.handle ? '@' + c.handle : '—'}</td>
              <td class="muted small">{formatDate(c.created_at)}</td>
              <td>{#if c.nudged_at}<span class="badge">convidado {formatDate(c.nudged_at)}</span>{:else}<span class="muted small">—</span>{/if}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
{/if}

<style>
  .funnel { display: flex; gap: 1rem; flex-wrap: wrap; margin-bottom: 1.5rem; }
  .stat { background: var(--surface-1, #fff); border: 1px solid var(--border-subtle, rgba(0,0,0,0.1)); border-radius: 12px; padding: 1rem 1.4rem; display: grid; gap: 0.15rem; min-width: 9rem; }
  .n { font-size: 1.8rem; font-weight: 800; font-variant-numeric: tabular-nums; }
  .n.hot { color: var(--accent, #15803d); }
  .toolbar { display: flex; gap: 0.6rem; align-items: center; flex-wrap: wrap; margin-bottom: 0.8rem; }
  .btn { padding: 0.5rem 1rem; border-radius: 8px; border: 1px solid var(--border-subtle, #ccc); background: var(--surface-1, #fff); color: inherit; font-weight: 600; cursor: pointer; }
  .btn.primary { background: var(--accent, #15803d); border-color: var(--accent, #15803d); color: var(--accent-contrast, #fff); }
  .btn.ghost { background: transparent; }
  .btn:disabled { opacity: 0.5; cursor: not-allowed; }
  .warn { color: #b45309; font-size: 0.85rem; }
  .result { font-weight: 600; margin: 0 0 1rem; }
  .card { background: var(--surface-1, #fff); border: 1px solid var(--border-subtle, rgba(0,0,0,0.1)); border-radius: 12px; padding: 1.5rem; }
  .err { color: #dc2626; }
  .table-wrap { overflow-x: auto; border: 1px solid var(--border-subtle, rgba(0,0,0,0.1)); border-radius: 12px; }
  table { width: 100%; border-collapse: collapse; font-size: 0.92rem; }
  th, td { text-align: left; padding: 0.6rem 0.8rem; border-bottom: 1px solid var(--border-subtle, rgba(0,0,0,0.06)); white-space: nowrap; }
  th { font-size: 0.78rem; text-transform: uppercase; letter-spacing: 0.03em; color: var(--muted, #64748b); }
  tr.nudged { opacity: 0.6; }
  .email { font-family: ui-monospace, monospace; }
  .small { font-size: 0.82rem; }
  .badge { font-size: 0.72rem; font-weight: 700; background: var(--surface-2, #f1f5f9); color: var(--muted, #64748b); padding: 0.15rem 0.5rem; border-radius: 999px; }
</style>
