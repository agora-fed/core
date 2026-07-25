<script lang="ts">
  // Fóruns institucionais (/f/...) — UMA ilha, três telas, roteamento client-side:
  //   /f/                → home (Federal | Estados | Governança)
  //   /f/<caminho>       → fórum (breadcrumb, seções, tópicos, novo tópico)
  //   /f/topico/<id>     → tópico (▲▼, comentários, recibos de envio)
  // O gateway serve f/index.html pra qualquer /f/* (SPA-fallback) — caminhos
  // materializados em runtime nunca dão 404 de build (lição das propostas).
  import { onMount } from 'svelte';
  import {
    commentForumTopic,
    createForumTopic,
    getForumTopic,
    getForumTopics,
    getForumTree,
    voteForumTopic,
    type ForumChildDto,
    type ForumDto,
    type ForumTopicDetailDto,
    type ForumTopicDto,
  } from '../../lib/api';

  type View = 'home' | 'forum' | 'topic';

  let view = $state<View>('home');
  let path = $state('');
  let topicId = $state('');

  let loading = $state(true);
  let error = $state<string | null>(null);

  // home
  let roots = $state<ForumChildDto[]>([]);
  // forum
  let forum = $state<ForumDto | null>(null);
  let children = $state<ForumChildDto[]>([]);
  let topics = $state<ForumTopicDto[]>([]);
  let sort = $state<'hot' | 'new'>('hot');
  let muniFilter = $state('');
  // topic
  let detail = $state<ForumTopicDetailDto | null>(null);
  // forms
  let title = $state('');
  let body = $state('');
  let comment = $state('');
  let busy = $state(false);
  let formMsg = $state<string | null>(null);

  function isLogged(): boolean {
    try {
      return Boolean(localStorage.getItem('dsoc_citizen'));
    } catch {
      return false;
    }
  }

  function parseLocation() {
    const raw = window.location.pathname.replace(/^\/f\/?/, '').replace(/\/$/, '');
    if (!raw) {
      view = 'home';
      path = '';
    } else if (raw.startsWith('topico/')) {
      view = 'topic';
      topicId = raw.slice('topico/'.length);
    } else {
      view = 'forum';
      path = raw;
    }
  }

  function navigate(to: string) {
    window.history.pushState({}, '', to);
    parseLocation();
    void load();
  }

  async function load() {
    loading = true;
    error = null;
    formMsg = null;
    // apiGet devolve o shape `Fetched` ({ok, data, error:string}) — diferente do
    // apiPost ({success, error:{message}}). Confundir os dois foi o bug do 1º deploy.
    if (view === 'home') {
      const res = await getForumTree();
      loading = false;
      if (res.ok && res.data) roots = res.data.children;
      else error = res.error ?? 'Não foi possível carregar os fóruns.';
    } else if (view === 'forum') {
      const [t, tp] = await Promise.all([
        getForumTree(path),
        getForumTopics(path, sort),
      ]);
      loading = false;
      if (t.ok && t.data) {
        forum = t.data.forum;
        children = t.data.children;
      } else {
        error = t.error ?? 'Fórum não encontrado.';
        return;
      }
      topics = tp.ok && tp.data ? tp.data.topics : [];
    } else {
      const res = await getForumTopic(topicId);
      loading = false;
      if (res.ok && res.data) detail = res.data;
      else error = res.error ?? 'Tópico não encontrado.';
    }
  }

  onMount(() => {
    parseLocation();
    void load();
    window.addEventListener('popstate', () => {
      parseLocation();
      void load();
    });
  });

  // --- home: agrupamento dos fóruns raiz ---
  const JUDICIARIO = new Set([
    'stf', 'stj', 'tst', 'tse', 'stm', 'cnj',
    'trf-1', 'trf-2', 'trf-3', 'trf-4', 'trf-5', 'trf-6',
  ]);
  let casas = $derived(roots.filter((r) => r.slug === 'senado' || r.slug === 'camara'));
  let ministerios = $derived(roots.filter((r) => r.slug.startsWith('ministerio-')));
  let judiciario = $derived(roots.filter((r) => JUDICIARIO.has(r.slug)));
  let governanca = $derived(roots.filter((r) => r.slug === 'governanca'));
  let estados = $derived(roots.filter((r) => r.slug.length === 2));

  // --- forum estado: separa seções padrão dos ~200 municípios ---
  let isEstado = $derived(
    forum?.esfera === 'estadual' && !(forum?.full_path ?? '').includes('/'),
  );
  let secoesEstado = $derived(isEstado ? children.filter((c) => c.virtual_section) : []);
  let municipios = $derived(isEstado ? children.filter((c) => !c.virtual_section) : []);
  let municipiosFiltrados = $derived(
    muniFilter.trim().length >= 2
      ? municipios.filter((m) =>
          m.name
            .toLocaleLowerCase('pt-BR')
            .includes(muniFilter.trim().toLocaleLowerCase('pt-BR')),
        )
      : municipios.slice(0, 30),
  );

  let crumbs = $derived(
    path
      ? path.split('/').map((seg, i, all) => ({
          seg,
          href: `/f/${all.slice(0, i + 1).join('/')}`,
        }))
      : [],
  );

  async function submitTopic(event: SubmitEvent) {
    event.preventDefault();
    if (busy || !title.trim() || !body.trim()) return;
    if (!isLogged()) {
      formMsg = 'Entre na sua conta para criar um tópico.';
      return;
    }
    busy = true;
    const res = await createForumTopic(path, title.trim(), body.trim());
    busy = false;
    if (res.success && res.data) {
      title = '';
      body = '';
      navigate(`/f/topico/${res.data.id}`);
    } else {
      formMsg = res.error?.message ?? 'Não foi possível criar o tópico.';
    }
  }

  async function vote(value: 1 | -1) {
    if (!detail || busy) return;
    if (!isLogged()) {
      formMsg = 'Entre na sua conta para votar.';
      return;
    }
    busy = true;
    const res = await voteForumTopic(detail.topic.id, value);
    busy = false;
    if (res.success && res.data) detail = { ...detail, topic: res.data };
    else formMsg = res.error?.message ?? 'Não foi possível votar.';
  }

  async function submitComment(event: SubmitEvent) {
    event.preventDefault();
    if (!detail || busy || !comment.trim()) return;
    if (!isLogged()) {
      formMsg = 'Entre na sua conta para comentar.';
      return;
    }
    busy = true;
    const res = await commentForumTopic(detail.topic.id, comment.trim());
    busy = false;
    if (res.success) {
      comment = '';
      void load();
    } else {
      formMsg = res.error?.message ?? 'Não foi possível comentar.';
    }
  }

  function fmtDate(s: string): string {
    return new Date(s).toLocaleString('pt-BR', {
      dateStyle: 'short',
      timeStyle: 'short',
    });
  }
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if error}
  <div class="card center" role="alert">
    <h2>{error}</h2>
    <p class="muted"><a href="/f/" onclick={(e) => { e.preventDefault(); navigate('/f/'); }}>Voltar aos fóruns</a></p>
  </div>
{:else if view === 'home'}
  <header class="f-head">
    <h1>🏛 Fóruns</h1>
    <p class="muted">
      Debata com a sociedade e leve o resultado às instituições: ao cruzar os
      patamares de interação, o fórum envia o debate — com as respostas mais
      votadas — para a comissão, ministério ou secretaria responsável, com
      recibo público.
    </p>
  </header>

  <section class="f-block">
    <h2>🇧🇷 Federal</h2>
    <div class="f-grid">
      {#each casas as c (c.slug)}
        <a class="f-card f-card-destaque" href={`/f/${c.full_path}`} onclick={(e) => { e.preventDefault(); navigate(`/f/${c.full_path}`); }}>{c.name}</a>
      {/each}
    </div>
    <h3>Ministérios</h3>
    <div class="f-grid">
      {#each ministerios as m (m.slug)}
        <a class="f-card" href={`/f/${m.full_path}`} onclick={(e) => { e.preventDefault(); navigate(`/f/${m.full_path}`); }}>{m.name}</a>
      {/each}
    </div>
    <h3>Judiciário</h3>
    <div class="f-grid">
      {#each judiciario as j (j.slug)}
        <a class="f-card" href={`/f/${j.full_path}`} onclick={(e) => { e.preventDefault(); navigate(`/f/${j.full_path}`); }}>{j.name}</a>
      {/each}
    </div>
  </section>

  <section class="f-block">
    <h2>🏛 Estados</h2>
    <div class="f-grid f-grid-uf">
      {#each estados as uf (uf.slug)}
        <a class="f-card" href={`/f/${uf.full_path}`} onclick={(e) => { e.preventDefault(); navigate(`/f/${uf.full_path}`); }}>{uf.name}</a>
      {/each}
    </div>
    <p class="muted small">Os municípios ficam dentro do estado — entre no seu estado e busque sua cidade.</p>
  </section>

  {#if governanca.length > 0}
    <section class="f-block">
      <h2>⚙️ Plataforma</h2>
      <div class="f-grid">
        {#each governanca as g (g.slug)}
          <a class="f-card" href={`/f/${g.full_path}`} onclick={(e) => { e.preventDefault(); navigate(`/f/${g.full_path}`); }}>{g.name}</a>
        {/each}
      </div>
    </section>
  {/if}
{:else if view === 'forum' && forum}
  <nav class="f-crumbs" aria-label="Caminho">
    <a href="/f/" onclick={(e) => { e.preventDefault(); navigate('/f/'); }}>Fóruns</a>
    {#each crumbs as c (c.href)}
      <span>›</span>
      <a href={c.href} onclick={(e) => { e.preventDefault(); navigate(c.href); }}>{c.seg}</a>
    {/each}
  </nav>

  <header class="f-head">
    <h1>{forum.name}</h1>
    {#if forum.description}<p class="muted">{forum.description}</p>{/if}
    {#if forum.has_contact_email}
      <p class="f-badge">✉️ Instituição com e-mail vinculado — patamares: {forum.thresholds.map((t) => t.toLocaleString('pt-BR')).join(' · ')} interações</p>
    {:else}
      <p class="f-badge muted">Patamares: {forum.thresholds.map((t) => t.toLocaleString('pt-BR')).join(' · ')} interações (e-mail institucional em curadoria)</p>
    {/if}
  </header>

  {#if isEstado}
    {#if secoesEstado.length > 0}
      <div class="f-chips">
        {#each secoesEstado as s (s.slug)}
          <a class="f-chip f-chip-virtual" href={`/f/${s.full_path}`} onclick={(e) => { e.preventDefault(); navigate(`/f/${s.full_path}`); }}>{s.name}</a>
        {/each}
      </div>
    {/if}
    <div class="field">
      <input class="input" type="search" placeholder="Buscar município…" bind:value={muniFilter} />
    </div>
    <div class="f-grid">
      {#each municipiosFiltrados as m (m.slug)}
        <a class="f-card" href={`/f/${m.full_path}`} onclick={(e) => { e.preventDefault(); navigate(`/f/${m.full_path}`); }}>{m.name}</a>
      {/each}
    </div>
    {#if muniFilter.trim().length < 2 && municipios.length > 30}
      <p class="muted small">Mostrando 30 de {municipios.length} municípios — use a busca.</p>
    {/if}
  {:else if children.length > 0}
    <div class="f-chips">
      {#each children as c (c.slug)}
        <a class="f-chip" class:f-chip-virtual={c.virtual_section} href={`/f/${c.full_path}`} onclick={(e) => { e.preventDefault(); navigate(`/f/${c.full_path}`); }}>{c.name}</a>
      {/each}
    </div>
  {/if}

  <section class="f-block">
    <div class="f-topics-head">
      <h2>Tópicos</h2>
      <div class="f-sort">
        <button type="button" class:active={sort === 'hot'} onclick={() => { sort = 'hot'; void load(); }}>Em alta</button>
        <button type="button" class:active={sort === 'new'} onclick={() => { sort = 'new'; void load(); }}>Recentes</button>
      </div>
    </div>
    {#if topics.length === 0}
      <p class="muted">Nenhum tópico ainda — seja a primeira pessoa a abrir o debate.</p>
    {/if}
    <ul class="f-topics">
      {#each topics as t (t.id)}
        <li>
          <a class="f-topic" href={`/f/topico/${t.id}`} onclick={(e) => { e.preventDefault(); navigate(`/f/topico/${t.id}`); }}>
            <span class="f-score" title="Saldo de votos">{t.score > 0 ? `+${t.score}` : t.score}</span>
            <span class="f-topic-main">
              <strong>{t.title}</strong>
              <span class="muted small">
                {t.interactions.toLocaleString('pt-BR')} interações
                {#if t.federated_interactions > 0}
                  · 🌐 {t.federated_interactions.toLocaleString('pt-BR')} federadas
                {/if}
                · {t.comment_count.toLocaleString('pt-BR')} comentários · {fmtDate(t.created_at)}
              </span>
            </span>
          </a>
        </li>
      {/each}
    </ul>
  </section>

  <section class="f-block">
    <h2>Novo tópico</h2>
    <form onsubmit={submitTopic}>
      <div class="field">
        <label for="ft-title">Título</label>
        <input id="ft-title" class="input" type="text" maxlength="200" bind:value={title} placeholder="Ex.: Distribuição de vacinas nos postos" />
      </div>
      <div class="field">
        <label for="ft-body">Descrição</label>
        <textarea id="ft-body" class="input" rows="4" bind:value={body} placeholder="Contexto, problema e o que você propõe debater…"></textarea>
      </div>
      <button class="btn btn-primary" type="submit" disabled={busy || !title.trim() || !body.trim()}>
        {busy ? 'Enviando…' : 'Criar tópico'}
      </button>
      {#if formMsg}<p class="note" role="status">{formMsg} {#if !isLogged()}<a href="/entrar">Entrar</a>{/if}</p>{/if}
    </form>
  </section>
{:else if view === 'topic' && detail}
  <nav class="f-crumbs" aria-label="Caminho">
    <a href="/f/" onclick={(e) => { e.preventDefault(); navigate('/f/'); }}>Fóruns</a>
    <span>›</span>
    <button type="button" class="f-linklike" onclick={() => window.history.back()}>← voltar</button>
  </nav>

  <article class="f-topic-page">
    <div class="f-vote-col">
      <button type="button" class="f-vote" aria-label="Voto a favor" onclick={() => vote(1)} disabled={busy}>▲</button>
      <span class="f-score-big">{detail.topic.score > 0 ? `+${detail.topic.score}` : detail.topic.score}</span>
      <button type="button" class="f-vote" aria-label="Voto contra" onclick={() => vote(-1)} disabled={busy}>▼</button>
    </div>
    <div class="f-topic-body">
      <h1>{detail.topic.title}</h1>
      <p class="muted small">
        por {detail.topic.author_public_handle} · {fmtDate(detail.topic.created_at)} ·
        <strong>{detail.topic.interactions.toLocaleString('pt-BR')} interações</strong>
        {#if detail.topic.federated_interactions > 0}
          · 🌐 {detail.topic.federated_interactions.toLocaleString('pt-BR')} federadas (não contam pro envio)
        {/if}
      </p>
      <div class="f-topic-text">{detail.topic.body}</div>

      {#if detail.dispatches.length > 0}
        <aside class="f-receipts" aria-label="Envios à instituição">
          {#each detail.dispatches as d (d.threshold)}
            <p>
              📨 Patamar de <strong>{d.threshold.toLocaleString('pt-BR')} interações</strong> cruzado em {fmtDate(d.crossed_at)}
              {#if d.sent_at}
                — e-mail enviado à instituição em {fmtDate(d.sent_at)}.
              {:else}
                — envio à instituição na fila.
              {/if}
            </p>
          {/each}
        </aside>
      {/if}

      <section class="f-comments">
        <h2>Comentários</h2>
        {#if detail.comments.length === 0}
          <p class="muted">Nenhum comentário ainda.</p>
        {/if}
        <ul>
          {#each detail.comments as c (c.id)}
            <li>
              <span class="muted small">
                {c.federated ? `🌐 ${c.author}` : c.author} · {fmtDate(c.created_at)}
              </span>
              <p>{c.body}</p>
            </li>
          {/each}
        </ul>
        <form onsubmit={submitComment}>
          <div class="field">
            <textarea class="input" rows="3" bind:value={comment} placeholder="Contribua com o debate…"></textarea>
          </div>
          <button class="btn btn-primary" type="submit" disabled={busy || !comment.trim()}>
            {busy ? 'Enviando…' : 'Comentar'}
          </button>
        </form>
        {#if formMsg}<p class="note" role="status">{formMsg} {#if !isLogged()}<a href="/entrar">Entrar</a>{/if}</p>{/if}
      </section>
    </div>
  </article>
{/if}

<style>
  .f-head { margin-bottom: 1.25rem; }
  .f-block { margin: 1.5rem 0; }
  .f-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(14rem, 1fr));
    gap: 0.5rem;
    margin: 0.5rem 0;
  }
  .f-grid-uf { grid-template-columns: repeat(auto-fill, minmax(11rem, 1fr)); }
  .f-card {
    display: block;
    padding: 0.7rem 0.9rem;
    border: 1px solid var(--c-border, #ccc);
    border-radius: 0.6rem;
    text-decoration: none;
    color: inherit;
    font-size: 0.95rem;
  }
  .f-card:hover { border-color: var(--c-primary, #2a9d54); }
  .f-card-destaque { font-weight: 700; }
  .f-chips { display: flex; flex-wrap: wrap; gap: 0.4rem; margin: 0.75rem 0; }
  .f-chip {
    padding: 0.25rem 0.75rem;
    border: 1px solid var(--c-border, #ccc);
    border-radius: 999px;
    text-decoration: none;
    color: inherit;
    font-size: 0.9rem;
  }
  .f-chip-virtual { opacity: 0.75; border-style: dashed; }
  .f-crumbs { display: flex; gap: 0.4rem; flex-wrap: wrap; font-size: 0.9rem; margin-bottom: 0.75rem; }
  .f-badge { font-size: 0.9rem; }
  .f-topics-head { display: flex; justify-content: space-between; align-items: baseline; gap: 1rem; }
  .f-sort button {
    background: none; border: 1px solid var(--c-border, #ccc);
    border-radius: 999px; padding: 0.15rem 0.7rem; cursor: pointer; color: inherit;
  }
  .f-sort button.active { border-color: var(--c-primary, #2a9d54); font-weight: 600; }
  .f-topics { list-style: none; margin: 0.5rem 0; padding: 0; }
  .f-topic {
    display: flex; gap: 0.9rem; align-items: center;
    padding: 0.6rem 0.4rem; border-bottom: 1px solid var(--c-border, #e3e3e3);
    text-decoration: none; color: inherit;
  }
  .f-score { min-width: 2.6rem; text-align: center; font-weight: 700; }
  .f-topic-main { display: flex; flex-direction: column; gap: 0.15rem; }
  .f-topic-page { display: flex; gap: 1rem; }
  .f-vote-col { display: flex; flex-direction: column; align-items: center; gap: 0.3rem; }
  .f-vote {
    border: 1px solid var(--c-border, #ccc); background: none; color: inherit;
    border-radius: 0.5rem; width: 2.4rem; height: 2.2rem; font-size: 1rem; cursor: pointer;
  }
  .f-vote:hover { border-color: var(--c-primary, #2a9d54); }
  .f-score-big { font-weight: 800; }
  .f-topic-body { flex: 1; min-width: 0; }
  .f-topic-text { white-space: pre-wrap; margin: 0.75rem 0 1rem; }
  .f-receipts {
    padding: 0.6rem 0.9rem; margin: 0.75rem 0;
    background: var(--c-green-soft, #e6f7ed);
    border: 1px solid #b7e4c7; border-radius: 0.6rem; font-size: 0.92rem;
  }
  .f-comments ul { list-style: none; padding: 0; }
  .f-comments li { border-bottom: 1px solid var(--c-border, #e3e3e3); padding: 0.5rem 0; }
  .f-linklike { background: none; border: none; color: inherit; cursor: pointer; padding: 0; font-size: 0.9rem; text-decoration: underline; }
  .note { margin-top: 0.5rem; font-size: 0.92rem; }
  .small { font-size: 0.85rem; }
</style>
