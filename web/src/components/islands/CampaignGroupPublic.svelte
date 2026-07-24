<script lang="ts">
  // Página pública do grupo de campanha. /grupo/?id=<uuid>
  // O eleitor vê a descrição + atualizações da campanha e entra/sai (join/leave).
  import { onMount } from 'svelte';
  import {
    getCampaignGroup,
    joinCampaignGroup,
    leaveCampaignGroup,
    respondCampaignPoll,
    type PublicCampaignGroup,
  } from '../../lib/api';

  let loading = $state(true);
  let error = $state<string | null>(null);
  let group = $state<PublicCampaignGroup | null>(null);
  let busy = $state(false);
  let busyPoll = $state<string | null>(null);
  let loggedIn = $state(false);

  function pct(n: number, total: number): number {
    return total > 0 ? Math.round((n / total) * 100) : 0;
  }

  async function answerPoll(pollId: string, choice: string) {
    if (!group || busyPoll) return;
    if (!loggedIn) {
      window.location.href = `/entrar/?next=${encodeURIComponent(window.location.pathname + window.location.search)}`;
      return;
    }
    busyPoll = pollId;
    const res = await respondCampaignPoll(group.id, pollId, choice);
    busyPoll = null;
    if (res.success) await load(group.id);
  }

  const fmtDate = new Intl.DateTimeFormat('pt-BR', { day: '2-digit', month: 'short', year: 'numeric' });

  async function load(id: string) {
    // getCampaignGroup/join/leave passam por apiGetCredentialed/apiPost →
    // shape { success, data, error:{message} } (não o { ok } do apiGet cru).
    const res = await getCampaignGroup(id);
    if (res.success && res.data) group = res.data;
    else error = res.error?.message ?? 'Grupo não encontrado.';
  }

  async function toggleMembership() {
    if (!group || busy) return;
    if (!loggedIn) {
      window.location.href = `/entrar/?next=${encodeURIComponent(window.location.pathname + window.location.search)}`;
      return;
    }
    busy = true;
    const res = group.sou_membro
      ? await leaveCampaignGroup(group.id)
      : await joinCampaignGroup(group.id);
    busy = false;
    if (res.success) await load(group.id);
  }

  onMount(async () => {
    try {
      loggedIn = Boolean(localStorage.getItem('dsoc_citizen'));
    } catch {
      loggedIn = false;
    }
    const id = new URLSearchParams(window.location.search).get('id') ?? '';
    if (!id) {
      error = 'Grupo não informado.';
      loading = false;
      return;
    }
    await load(id);
    loading = false;
  });
</script>

{#if loading}
  <p class="muted">Carregando grupo…</p>
{:else if error || !group}
  <div class="card center">
    <p class="hint-error" role="alert">{error ?? 'Grupo não encontrado.'}</p>
    <p class="muted"><a href="/politicos">Ver políticos</a></p>
  </div>
{:else}
  <header class="head">
    <h1>{group.name}</h1>
    <p class="owner muted">
      Grupo de campanha de
      {#if group.owner_handle}
        <a href={`/perfil/?u=${encodeURIComponent(group.owner_handle)}`}>{group.owner_display_name ?? '@' + group.owner_handle}</a>
      {:else}
        <a href={`/politicos/${group.mandate_id}`}>{group.owner_display_name ?? 'político(a)'}</a>
      {/if}
    </p>
    {#if group.description}
      <p class="desc">{group.description}</p>
    {/if}
    <div class="cta-row">
      <button
        class="btn"
        class:joined={group.sou_membro}
        onclick={toggleMembership}
        disabled={busy}
      >
        {busy ? '…' : group.sou_membro ? '✓ Você participa — sair' : 'Participar da campanha'}
      </button>
      <span class="members muted">{group.member_count} apoiador{group.member_count === 1 ? '' : 'es'}</span>
    </div>
  </header>

  {#if group.polls.length > 0}
    <section class="enquetes">
      <h2>Enquetes da campanha</h2>
      {#each group.polls as poll (poll.id)}
        <div class="card poll">
          <div class="poll-head">
            <p class="poll-q">{poll.question}</p>
            {#if poll.status === 'closed'}<span class="badge closed">Encerrada</span>{/if}
          </div>
          {#if poll.status === 'open'}
            <div class="options" role="group" aria-label="Sua resposta">
              {#each [['concordo','Concordo'],['neutro','Neutro'],['discordo','Discordo']] as [key, label] (key)}
                <button
                  type="button"
                  class="opt {key}"
                  class:selected={poll.my_answer === key}
                  aria-pressed={poll.my_answer === key}
                  disabled={busyPoll === poll.id}
                  onclick={() => answerPoll(poll.id, key)}
                >{label}</button>
              {/each}
            </div>
          {/if}
          <div class="results">
            {#each [['concordo','Concordo'],['neutro','Neutro'],['discordo','Discordo']] as [key, label] (key)}
              <div class="bar-row">
                <span class="bar-label">{label}</span>
                <div class="bar-track"><div class="bar-fill {key}" style={`width:${pct(poll.tally[key], poll.tally.total)}%`}></div></div>
                <span class="bar-val muted">{pct(poll.tally[key], poll.tally.total)}% · {poll.tally[key]}</span>
              </div>
            {/each}
            <p class="total muted">{poll.tally.total} {poll.tally.total === 1 ? 'resposta' : 'respostas'}</p>
          </div>
        </div>
      {/each}
    </section>
  {/if}

  <section class="updates">
    <h2>Atualizações</h2>
    {#if group.posts.length === 0}
      <p class="muted">A campanha ainda não publicou nada.</p>
    {:else}
      <ul class="posts">
        {#each group.posts as p (p.id)}
          <li class="card post">
            <p class="post-body">{p.body}</p>
            <time class="muted">{fmtDate.format(new Date(p.created_at))}</time>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
{/if}

<style>
  .head { margin-bottom: 2rem; }
  .head h1 { margin: 0 0 0.3rem; font-size: 1.8rem; }
  .owner { margin: 0 0 0.75rem; }
  .owner a { color: var(--accent-strong, #15803d); text-decoration: none; }
  .owner a:hover { text-decoration: underline; }
  .desc { margin: 0 0 1.2rem; font-size: 1.05rem; color: var(--text-2, inherit); }
  .cta-row { display: flex; align-items: center; gap: 1rem; flex-wrap: wrap; }
  .btn {
    padding: 0.6rem 1.3rem;
    border-radius: 10px;
    border: none;
    background: var(--accent, #15803d);
    color: var(--accent-contrast, #fff);
    font-weight: 700;
    font-size: 1rem;
    cursor: pointer;
    transition: filter 100ms ease;
  }
  .btn:hover:not(:disabled) { filter: brightness(1.05); }
  .btn:disabled { opacity: 0.6; cursor: default; }
  .btn.joined {
    background: var(--surface-2, #eef);
    color: var(--text-1, inherit);
    border: 1px solid var(--border-subtle, #ccc);
  }
  .members { font-variant-numeric: tabular-nums; font-weight: 600; }

  .updates h2 { font-size: 1.2rem; margin: 0 0 1rem; }
  .posts { list-style: none; padding: 0; margin: 0; display: grid; gap: 0.8rem; }
  .post { padding: 1rem 1.2rem; }
  .post-body { margin: 0 0 0.5rem; white-space: pre-wrap; }
  .post time { font-size: 0.8rem; }
  .card {
    background: var(--surface-1, #fff);
    border: 1px solid var(--border-subtle, rgba(0,0,0,0.1));
    border-radius: 12px;
  }
  .center { text-align: center; padding: 2.5rem 1.5rem; }
  .hint-error { color: #dc2626; }

  .enquetes { margin-bottom: 2rem; }
  .enquetes h2 { font-size: 1.2rem; margin: 0 0 1rem; }
  .poll { padding: 1.1rem 1.2rem; display: grid; gap: 0.85rem; margin-bottom: 0.8rem; }
  .poll-head { display: flex; justify-content: space-between; align-items: start; gap: 0.75rem; }
  .poll-q { margin: 0; font-weight: 600; font-size: 1.05rem; }
  .badge.closed { font-size: 0.68rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.03em; padding: 0.15rem 0.5rem; border-radius: 999px; background: var(--surface-2, #f1f5f9); color: var(--text-2, #64748b); white-space: nowrap; }
  .options { display: flex; gap: 0.5rem; flex-wrap: wrap; }
  .opt { flex: 1 1 auto; min-width: 6rem; padding: 0.55rem 0.7rem; border-radius: 8px; border: 1.5px solid var(--border-subtle, #cbd5e1); background: var(--surface-1, #fff); color: inherit; font-weight: 600; cursor: pointer; }
  .opt:hover:not(:disabled) { border-color: var(--text-1, #0f172a); }
  .opt:disabled { opacity: 0.6; cursor: default; }
  .opt.selected.concordo { border-color: #15803d; background: #dcfce7; color: #14532d; }
  .opt.selected.neutro { border-color: #b45309; background: #fef3c7; color: #78350f; }
  .opt.selected.discordo { border-color: #b91c1c; background: #fee2e2; color: #7f1d1d; }
  .results { display: grid; gap: 0.35rem; }
  .bar-row { display: grid; grid-template-columns: 4.5rem 1fr auto; align-items: center; gap: 0.5rem; font-size: 0.85rem; }
  .bar-label { color: var(--text-2, #64748b); }
  .bar-track { height: 0.55rem; background: var(--surface-2, #f1f5f9); border-radius: 999px; overflow: hidden; }
  .bar-fill { height: 100%; border-radius: 999px; min-width: 2px; }
  .bar-fill.concordo { background: #22c55e; }
  .bar-fill.neutro { background: #f59e0b; }
  .bar-fill.discordo { background: #ef4444; }
  .bar-val { font-variant-numeric: tabular-nums; }
  .total { margin: 0.15rem 0 0; font-size: 0.8rem; }
</style>
