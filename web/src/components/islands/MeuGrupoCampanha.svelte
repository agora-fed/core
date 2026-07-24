<script lang="ts">
  // Painel do dono do grupo de campanha (/meu-grupo). Só político (vínculo de
  // mandato) — cria/edita o grupo, publica atualizações e vê os apoiadores.
  import { onMount } from 'svelte';
  import {
    getMyCampaignGroup,
    upsertCampaignGroup,
    postCampaignGroupUpdate,
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
</style>
