<script lang="ts">
  // Painel do dono do grupo de campanha (/meu-grupo). Só político (vínculo de
  // mandato) — cria/edita o grupo, publica atualizações e vê os apoiadores.
  import { onMount } from 'svelte';
  import {
    getMyCampaignGroup,
    upsertCampaignGroup,
    postCampaignGroupUpdate,
    createCampaignPoll,
    closeCampaignPoll,
    type MyCampaignGroup,
  } from '../../lib/api';

  let loading = $state(true);
  let data = $state<MyCampaignGroup | null>(null);

  // Form de criação/edição.
  let name = $state('');
  let description = $state('');
  let savingGroup = $state(false);
  let groupError = $state<string | null>(null);

  // Form de post.
  let postBody = $state('');
  let posting = $state(false);
  let postError = $state<string | null>(null);

  // Form de enquete.
  let pollQuestion = $state('');
  let creatingPoll = $state(false);
  let pollError = $state<string | null>(null);

  function pct(n: number, total: number): number {
    return total > 0 ? Math.round((n / total) * 100) : 0;
  }

  const fmtDate = new Intl.DateTimeFormat('pt-BR', { day: '2-digit', month: 'short', year: 'numeric' });

  let shareUrl = $derived(
    data?.group ? `${location.origin}/grupo/?id=${data.group.id}` : '',
  );

  async function reload() {
    // Tudo aqui passa por apiGetCredentialed/apiPost → shape { success, data, error:{message} }.
    const res = await getMyCampaignGroup();
    if (res.success && res.data) {
      data = res.data;
      if (data.group) {
        name = data.group.name;
        description = data.group.description ?? '';
      }
    }
  }

  async function saveGroup(e: SubmitEvent) {
    e.preventDefault();
    if (name.trim().length === 0 || savingGroup) return;
    savingGroup = true;
    groupError = null;
    const res = await upsertCampaignGroup(name.trim(), description.trim() || undefined);
    savingGroup = false;
    if (res.success) await reload();
    else groupError = res.error?.message ?? 'Não foi possível salvar.';
  }

  async function publish(e: SubmitEvent) {
    e.preventDefault();
    if (postBody.trim().length === 0 || posting) return;
    posting = true;
    postError = null;
    const res = await postCampaignGroupUpdate(postBody.trim());
    posting = false;
    if (res.success) {
      postBody = '';
      await reload();
    } else {
      postError = res.error?.message ?? 'Não foi possível publicar.';
    }
  }

  async function openPoll(e: SubmitEvent) {
    e.preventDefault();
    if (pollQuestion.trim().length === 0 || creatingPoll) return;
    creatingPoll = true;
    pollError = null;
    const res = await createCampaignPoll(pollQuestion.trim());
    creatingPoll = false;
    if (res.success) {
      pollQuestion = '';
      await reload();
    } else {
      pollError = res.error?.message ?? 'Não foi possível abrir a enquete.';
    }
  }

  async function endPoll(id: string) {
    const res = await closeCampaignPoll(id);
    if (res.success) await reload();
  }

  onMount(async () => {
    await reload();
    loading = false;
  });
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if !data?.is_politico}
  <div class="card center">
    <h2>Exclusivo de mandatos</h2>
    <p class="muted">
      O grupo de campanha é uma ferramenta para políticos e candidatos vinculados.
      <a href="/cadastrar">Cadastre sua candidatura</a> para ativá-lo.
    </p>
  </div>
{:else}
  <header class="head">
    <h1>Meu grupo de campanha</h1>
    <p class="muted">Seu canal direto com os eleitores: publique atualizações e reúna apoiadores.</p>
  </header>

  <form class="card block" onsubmit={saveGroup}>
    <h2>{data.group ? 'Editar grupo' : 'Criar meu grupo'}</h2>
    <label>
      <span>Nome do grupo</span>
      <input type="text" bind:value={name} maxlength="80" placeholder="Ex.: Campanha da Fulana 2026" />
    </label>
    <label>
      <span>Descrição</span>
      <textarea bind:value={description} maxlength="500" rows="3" placeholder="Conte em uma frase o que move sua campanha."></textarea>
    </label>
    {#if groupError}<p class="hint-error" role="alert">{groupError}</p>{/if}
    <button type="submit" class="btn-primary" disabled={savingGroup || name.trim().length === 0}>
      {savingGroup ? 'Salvando…' : data.group ? 'Salvar' : 'Criar grupo'}
    </button>
  </form>

  {#if data.group}
    <div class="stats-row">
      <div class="stat card">
        <span class="stat-val">{data.member_count}</span>
        <span class="muted">apoiador{data.member_count === 1 ? '' : 'es'}</span>
      </div>
      <div class="share card">
        <span class="muted">Link para divulgar:</span>
        <a href={shareUrl}>{shareUrl}</a>
      </div>
    </div>

    <form class="card block" onsubmit={publish}>
      <h2>Publicar atualização</h2>
      <textarea bind:value={postBody} maxlength="2000" rows="4" placeholder="O que você quer contar aos seus apoiadores?"></textarea>
      {#if postError}<p class="hint-error" role="alert">{postError}</p>{/if}
      <button type="submit" class="btn-primary" disabled={posting || postBody.trim().length === 0}>
        {posting ? 'Publicando…' : 'Publicar'}
      </button>
    </form>

    <section class="polls-block">
      <form class="card block" onsubmit={openPoll}>
        <h2>Abrir enquete</h2>
        <p class="muted small">Pergunte à sua base e ouça a resposta. Cada apoiador responde concordo, neutro ou discordo.</p>
        <input type="text" bind:value={pollQuestion} maxlength="300" placeholder="Ex.: Devo priorizar saúde no orçamento?" />
        {#if pollError}<p class="hint-error" role="alert">{pollError}</p>{/if}
        <button type="submit" class="btn-primary" disabled={creatingPoll || pollQuestion.trim().length === 0}>
          {creatingPoll ? 'Abrindo…' : 'Abrir enquete'}
        </button>
      </form>

      {#if data.polls.length > 0}
        <ul class="polls">
          {#each data.polls as poll (poll.id)}
            <li class="card poll">
              <div class="poll-head">
                <p class="poll-q">{poll.question}</p>
                <span class="badge {poll.status}">{poll.status === 'open' ? 'Aberta' : 'Encerrada'}</span>
              </div>
              <div class="results">
                {#each [['concordo','Concordo'],['neutro','Neutro'],['discordo','Discordo']] as [key, label] (key)}
                  <div class="bar-row">
                    <span class="bar-label">{label}</span>
                    <div class="bar-track"><div class="bar-fill {key}" style={`width:${pct(poll.tally[key], poll.tally.total)}%`}></div></div>
                    <span class="bar-val muted">{pct(poll.tally[key], poll.tally.total)}% · {poll.tally[key]}</span>
                  </div>
                {/each}
                <p class="total muted small">{poll.tally.total} {poll.tally.total === 1 ? 'resposta' : 'respostas'}</p>
              </div>
              {#if poll.status === 'open'}
                <button class="btn-close" onclick={() => endPoll(poll.id)}>Encerrar enquete</button>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    {#if data.posts.length > 0}
      <section class="published">
        <h2>Publicadas</h2>
        <ul class="posts">
          {#each data.posts as p (p.id)}
            <li class="card post">
              <p class="post-body">{p.body}</p>
              <time class="muted">{fmtDate.format(new Date(p.created_at))}</time>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  {/if}
{/if}

<style>
  .head { margin-bottom: 1.5rem; }
  .head h1 { margin: 0 0 0.3rem; font-size: 1.7rem; }
  .block { padding: 1.25rem; margin-bottom: 1.5rem; display: grid; gap: 0.85rem; }
  .block h2 { margin: 0; font-size: 1.15rem; }
  label { display: grid; gap: 0.3rem; font-weight: 600; font-size: 0.9rem; }
  input, textarea {
    padding: 0.6rem 0.7rem;
    border-radius: 8px;
    border: 1px solid var(--border-subtle, #ccc);
    background: var(--surface-1, #fff);
    color: var(--text-1, inherit);
    font: inherit;
    resize: vertical;
  }
  .btn-primary {
    justify-self: start;
    padding: 0.55rem 1.2rem;
    border-radius: 8px;
    border: none;
    background: var(--accent, #15803d);
    color: var(--accent-contrast, #fff);
    font-weight: 700;
    cursor: pointer;
  }
  .btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }

  .stats-row { display: grid; grid-template-columns: auto 1fr; gap: 1rem; margin-bottom: 1.5rem; }
  .stat { padding: 1rem 1.4rem; text-align: center; display: grid; gap: 0.15rem; }
  .stat-val { font-size: 1.8rem; font-weight: 800; font-variant-numeric: tabular-nums; }
  .share { padding: 1rem 1.2rem; display: grid; gap: 0.25rem; align-content: center; min-width: 0; }
  .share a { color: var(--accent-strong, #15803d); word-break: break-all; font-size: 0.9rem; }

  .published h2 { font-size: 1.15rem; margin: 0 0 0.8rem; }
  .posts { list-style: none; padding: 0; margin: 0; display: grid; gap: 0.7rem; }
  .post { padding: 1rem 1.2rem; }
  .post-body { margin: 0 0 0.4rem; white-space: pre-wrap; }
  .post time { font-size: 0.8rem; }
  .card {
    background: var(--surface-1, #fff);
    border: 1px solid var(--border-subtle, rgba(0,0,0,0.1));
    border-radius: 12px;
  }
  .center { text-align: center; padding: 2.5rem 1.5rem; }
  .center h2 { margin: 0 0 0.5rem; }
  .hint-error { color: #dc2626; margin: 0; }
  .small { font-weight: 400; font-size: 0.82rem; }

  .polls-block { margin-bottom: 1.5rem; }
  .polls { list-style: none; padding: 0; margin: 1rem 0 0; display: grid; gap: 0.8rem; }
  .poll { padding: 1.1rem 1.2rem; display: grid; gap: 0.8rem; }
  .poll-head { display: flex; justify-content: space-between; align-items: start; gap: 0.75rem; }
  .poll-q { margin: 0; font-weight: 600; }
  .badge { font-size: 0.68rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.03em; padding: 0.15rem 0.5rem; border-radius: 999px; white-space: nowrap; }
  .badge.open { background: #dcfce7; color: #15803d; }
  .badge.closed { background: #f1f5f9; color: #64748b; }
  .results { display: grid; gap: 0.35rem; }
  .bar-row { display: grid; grid-template-columns: 4.5rem 1fr auto; align-items: center; gap: 0.5rem; font-size: 0.85rem; }
  .bar-label { color: var(--text-2, #64748b); }
  .bar-track { height: 0.55rem; background: var(--surface-2, #f1f5f9); border-radius: 999px; overflow: hidden; }
  .bar-fill { height: 100%; border-radius: 999px; min-width: 2px; }
  .bar-fill.concordo { background: #22c55e; }
  .bar-fill.neutro { background: #f59e0b; }
  .bar-fill.discordo { background: #ef4444; }
  .bar-val { font-variant-numeric: tabular-nums; }
  .total { margin: 0.15rem 0 0; }
  .btn-close { justify-self: start; padding: 0.4rem 0.9rem; border-radius: 8px; border: 1px solid var(--border-subtle, #ccc); background: transparent; color: inherit; font-weight: 600; cursor: pointer; font-size: 0.85rem; }
</style>
