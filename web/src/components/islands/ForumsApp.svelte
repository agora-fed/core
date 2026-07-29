<script lang="ts">
  // Fóruns institucionais (/f/...) — UMA ilha, três telas, roteamento client-side:
  //   /f/                → home (Federal | Estados | Governança)
  //   /f/<caminho>       → fórum (breadcrumb, seções, tópicos, novo tópico)
  //   /f/topico/<id>     → tópico (▲▼, comentários, recibos de envio)
  // O gateway serve f/index.html pra qualquer /f/* (SPA-fallback) — caminhos
  // materializados em runtime nunca dão 404 de build (lição das propostas).
  import { onMount } from 'svelte';
  import {
    adminListForums,
    adminUpdateForum,
    commentForumTopic,
    createForumTopic,
    getForumTopic,
    getForumTopics,
    getForumTree,
    getRecentForumTopics,
    getMyPermissions,
    moderateRemoveTopic,
    moderateRemoveComment,
    postNote,
    reportNote,
    voteForumComment,
    voteForumTopic,
    type ForumStance,
    type RecentForumTopicDto,
    type ForumChildDto,
    type ForumCommentItemDto,
    type ForumDto,
    type ForumTopicDetailDto,
    type ForumTopicDto,
  } from '../../lib/api';
  import { mdToHtml, titleSlug } from '../../lib/markdown';
  import { toast } from '../../lib/toasts';
  import Modal from '../ui/Modal.svelte';
  import Button from '../ui/Button.svelte';

  type View = 'home' | 'forum' | 'topic';

  let view = $state<View>('home');
  let path = $state('');
  let topicId = $state('');

  let loading = $state(true);
  let error = $state<string | null>(null);

  // home
  let roots = $state<ForumChildDto[]>([]);
  let recent = $state<RecentForumTopicDto[]>([]);
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
  // Posição escolhida no composer (modelo do debate: argumento + posição juntos).
  let commentStance = $state<ForumStance>('favor');
  let busy = $state(false);
  let formMsg = $state<string | null>(null);
  let showPreview = $state(false);
  // Moderação (R3.1 #27): mostra os botões de remover só pra quem pode moderar
  // globalmente (content.moderate/forums.moderate/administrator). Moderador só-de-um-
  // fórum ainda age via API mas não ganha o botão nesta fatia; o backend reenforça.
  let canModerate = $state(false);

  // Denúncia (issue #20) — reusa POST /me/reports, que é genérico por object_uri.
  let reportOpen = $state(false);
  let reportTarget = $state<{ object_uri: string; author_actor_url: string } | null>(null);
  let reportCategory = $state<'spam' | 'violation' | 'other'>('spam');
  let reportReason = $state('');
  let reportBusy = $state(false);
  // Admin inline (a capacidade é provada pela própria API admin responder).
  let isAdmin = $state(false);
  let admForumId = $state('');
  let admEmail = $state('');
  let admThresholds = $state('');
  let admAvatar = $state('');
  let admBanner = $state('');
  let admMsg = $state<string | null>(null);

  async function loadAdminInline(fullPath: string) {
    isAdmin = false;
    admMsg = null;
    const res = await adminListForums(fullPath, 0, 200);
    if (res.success && res.data) {
      const row = res.data.find((r) => r.full_path === fullPath);
      if (row) {
        isAdmin = true;
        admForumId = row.id;
        admEmail = row.contact_email ?? '';
        admThresholds = row.thresholds.join(', ');
        admAvatar = row.avatar_url ?? '';
        admBanner = row.banner_url ?? '';
      }
    }
  }

  async function saveAdminInline() {
    if (busy) return;
    const parts = admThresholds
      .split(/[,\s]+/)
      .filter(Boolean)
      .map(Number);
    const ok =
      parts.length > 0 &&
      parts.every((n) => Number.isInteger(n) && n > 0) &&
      parts.every((n, i) => i === 0 || n > parts[i - 1]);
    if (!ok) {
      admMsg = 'Patamares inválidos — inteiros crescentes (ex.: 1000, 10000, 100000).';
      return;
    }
    busy = true;
    const res = await adminUpdateForum(admForumId, {
      contact_email: admEmail.trim(),
      thresholds: parts,
      avatar_url: admAvatar.trim(),
      banner_url: admBanner.trim(),
    });
    busy = false;
    admMsg = res.success ? '✅ Salvo.' : (res.error?.message ?? 'Falha ao salvar.');
    if (res.success) void load();
  }

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
      // /f/topico/<uuid>[/<slug-do-titulo>] — o slug é cosmético/SEO, ignorado.
      topicId = raw.split('/')[1] ?? '';
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
      const [res, rec] = await Promise.all([
        getForumTree(),
        getRecentForumTopics(25),
      ]);
      loading = false;
      if (res.ok && res.data) roots = res.data.children;
      else error = res.error ?? 'Não foi possível carregar os fóruns.';
      recent = rec.ok && rec.data ? rec.data : [];
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
      void loadAdminInline(path);
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
    void loadModeration();
    window.addEventListener('popstate', () => {
      parseLocation();
      void load();
    });
  });

  async function loadModeration() {
    if (!isLogged()) return;
    const res = await getMyPermissions();
    if (res.success && res.data) {
      const k = res.data.keys;
      canModerate =
        res.data.is_administrator ||
        k.includes('content.moderate') ||
        k.includes('forums.moderate');
    }
  }

  async function removeTopic() {
    if (!detail || !canModerate) return;
    const reason =
      window.prompt('Remover este tópico (moderação). Motivo (opcional):') ?? undefined;
    const res = await moderateRemoveTopic(detail.topic.id, reason);
    if (res.success) {
      toast.success('Tópico removido.');
      navigate(path ? `/f/${path}` : '/f/'); // volta pra lista do fórum
    } else {
      toast.error(res.error?.message ?? 'Não foi possível remover.');
    }
  }

  async function removeComment(c: ForumCommentItemDto) {
    if (!detail || !canModerate) return;
    const reason =
      window.prompt('Remover este argumento (moderação). Motivo (opcional):') ?? undefined;
    const res = await moderateRemoveComment(c.id, reason);
    if (res.success) {
      toast.success('Argumento removido.');
      detail = { ...detail, comments: detail.comments.filter((x) => x.id !== c.id) };
    } else {
      toast.error(res.error?.message ?? 'Não foi possível remover.');
    }
  }

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

  async function vote(stance: ForumStance) {
    if (!detail || busy) return;
    if (!isLogged()) {
      formMsg = 'Entre na sua conta para se posicionar.';
      return;
    }
    busy = true;
    const res = await voteForumTopic(detail.topic.id, stance);
    busy = false;
    if (res.success && res.data) detail = { ...detail, topic: res.data };
    else formMsg = res.error?.message ?? 'Não foi possível registrar a posição.';
  }

  // Colunas do debate (ADR-0019: só A favor / Contra; ponderação eliminada).
  const STANCES: { key: ForumStance; label: string }[] = [
    { key: 'favor', label: 'A favor' },
    { key: 'contra', label: 'Contra' },
  ];
  function columnComments(stance: ForumStance): ForumCommentItemDto[] {
    if (!detail) return [];
    // Estilo StackOverflow: dentro da coluna, o argumento mais votado sobe.
    return detail.comments
      .filter((c) => (c.federated ? false : (c.stance ?? 'favor') === stance))
      .toSorted((a, b) => b.favor - b.contra - (a.favor - a.contra) || (a.id < b.id ? -1 : 1));
  }

  async function voteArg(c: ForumCommentItemDto, stance: ForumStance) {
    if (!detail || busy) return;
    if (!isLogged()) {
      formMsg = 'Entre na sua conta para votar num argumento.';
      return;
    }
    busy = true;
    const res = await voteForumComment(c.id, stance);
    busy = false;
    const data = res.success ? res.data : null;
    if (data) {
      detail = {
        ...detail,
        topic: data.topic,
        comments: detail.comments.map((x) => (x.id === c.id ? data.comment : x)),
      };
    } else {
      formMsg = res.error?.message ?? 'Não foi possível votar no argumento.';
    }
  }
  // URL do ator a partir do handle: local → /actors/{h}; federado (user@host) →
  // convenção Mastodon https://{host}/users/{user} (mesma heurística do backend).
  function actorUrlFromAuthor(handle: string): string {
    const h = handle.replace(/^@/, '');
    const at = h.lastIndexOf('@');
    if (at > 0) return `https://${h.slice(at + 1)}/users/${h.slice(0, at)}`;
    return `${location.origin}/actors/${h}`;
  }

  function askReportTopic() {
    if (!detail) return;
    if (!isLogged()) {
      formMsg = 'Entre para denunciar.';
      return;
    }
    reportTarget = {
      object_uri: `${location.origin}/f/topico/${detail.topic.id}`,
      author_actor_url: actorUrlFromAuthor(detail.topic.author_public_handle),
    };
    reportCategory = 'spam';
    reportReason = '';
    reportOpen = true;
  }

  function askReportComment(c: ForumCommentItemDto) {
    if (!detail) return;
    if (!isLogged()) {
      formMsg = 'Entre para denunciar.';
      return;
    }
    reportTarget = {
      object_uri: `${location.origin}/f/topico/${detail.topic.id}#arg-${c.id}`,
      author_actor_url: actorUrlFromAuthor(c.author),
    };
    reportCategory = 'spam';
    reportReason = '';
    reportOpen = true;
  }

  async function submitReport() {
    if (!reportTarget || reportBusy) return;
    reportBusy = true;
    const res = await reportNote({
      object_uri: reportTarget.object_uri,
      author_actor_url: reportTarget.author_actor_url,
      category: reportCategory,
      reason: reportReason.trim() || undefined,
    });
    reportBusy = false;
    if (res.success) {
      toast.success('Denúncia enviada à moderação.');
      reportOpen = false;
      reportTarget = null;
    } else {
      toast.error(res.error?.message ?? 'Falha ao enviar denúncia.');
    }
  }

  let federatedComments = $derived(
    detail ? detail.comments.filter((c) => c.federated) : [],
  );

  let shareState = $state<'idle' | 'sending' | 'sent' | 'copied' | 'failed'>('idle');
  let shareApUrl = $state('');

  function topicUrl(): string {
    return window.location.origin + window.location.pathname;
  }

  /// Compartilha pelo PRÓPRIO ator federado do usuário: publica uma Note pública
  /// com o link — seguidores em Mastodon/Lemmy/etc. recebem; o link abre com
  /// preview (OG). Fase F4 (fórum como ator Group próprio) vem depois.
  async function shareFediverso() {
    if (!detail || shareState === 'sending') return;
    if (!isLogged()) {
      formMsg = 'Entre na sua conta para compartilhar no fediverso.';
      return;
    }
    shareState = 'sending';
    const res = await postNote(
      `📣 "${detail.topic.title}"\n\nDebate público aberto na DemocraciaBR — participe:\n${topicUrl()}`,
    );
    shareState = res.success ? 'sent' : 'failed';
    // URL AP do objeto: seguidores recebem sozinhos; pra IMPORTAR/impulsionar de
    // uma conta Mastodon que ainda não seguia, cola-se esta URL na busca de lá.
    shareApUrl =
      res.success && res.data
        ? res.data.activity_id.replace('/activities/note-', '/objects/')
        : '';
  }

  async function copyLink() {
    try {
      await navigator.clipboard.writeText(topicUrl());
      shareState = 'copied';
    } catch {
      shareState = 'failed';
    }
  }

  async function submitComment(event: SubmitEvent) {
    event.preventDefault();
    if (!detail || busy || !comment.trim()) return;
    if (!isLogged()) {
      formMsg = 'Entre na sua conta para comentar.';
      return;
    }
    busy = true;
    const res = await commentForumTopic(detail.topic.id, comment.trim(), commentStance);
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

  <div class="f-home">
    <aside class="f-side" aria-label="Todos os fóruns">
      <details open>
        <summary>🇧🇷 Casas Federais</summary>
        <nav>
          {#each casas as c (c.slug)}
            <a href={`/f/${c.full_path}`} onclick={(e) => { e.preventDefault(); navigate(`/f/${c.full_path}`); }}>{c.name}</a>
          {/each}
        </nav>
      </details>
      <details>
        <summary>🏢 Ministérios <span class="muted">({ministerios.length})</span></summary>
        <nav>
          {#each ministerios as m (m.slug)}
            <a href={`/f/${m.full_path}`} onclick={(e) => { e.preventDefault(); navigate(`/f/${m.full_path}`); }}>{m.name}</a>
          {/each}
        </nav>
      </details>
      <details>
        <summary>⚖️ Judiciário <span class="muted">({judiciario.length})</span></summary>
        <nav>
          {#each judiciario as j (j.slug)}
            <a href={`/f/${j.full_path}`} onclick={(e) => { e.preventDefault(); navigate(`/f/${j.full_path}`); }}>{j.name}</a>
          {/each}
        </nav>
      </details>
      <details>
        <summary>🏛 Estados <span class="muted">(27 — municípios dentro de cada um)</span></summary>
        <nav>
          {#each estados as uf (uf.slug)}
            <a href={`/f/${uf.full_path}`} onclick={(e) => { e.preventDefault(); navigate(`/f/${uf.full_path}`); }}>{uf.name}</a>
          {/each}
        </nav>
      </details>
      {#if governanca.length > 0}
        <details>
          <summary>⚙️ Plataforma</summary>
          <nav>
            {#each governanca as g (g.slug)}
              <a href={`/f/${g.full_path}`} onclick={(e) => { e.preventDefault(); navigate(`/f/${g.full_path}`); }}>{g.name}</a>
            {/each}
          </nav>
        </details>
      {/if}
    </aside>

    <section class="f-feed" aria-label="Últimas postagens">
      <h2>Últimas postagens</h2>
      {#if recent.length === 0}
        <p class="muted">Nenhuma postagem ainda — escolha um fórum à esquerda e abra o primeiro debate.</p>
      {/if}
      <ul class="f-topics">
        {#each recent as t (t.id)}
          <li>
            <a class="f-topic" href={`/f/topico/${t.id}/${titleSlug(t.title)}`} onclick={(e) => { e.preventDefault(); navigate(`/f/topico/${t.id}/${titleSlug(t.title)}`); }}>
              <span class="f-tally" title="A favor · Contra · Ponderações">
                <span class="f-t-favor">{t.favor}</span>
                <span class="f-t-contra">{t.contra}</span>
              </span>
              <span class="f-topic-main">
                <strong>{t.title}</strong>
                <span class="muted small">
                  <span class="f-feed-badge">/f/{t.forum_path}</span>
                  · {t.interactions.toLocaleString('pt-BR')} interações
                  · {t.comment_count.toLocaleString('pt-BR')} comentários
                  · {fmtDate(t.created_at)}
                </span>
              </span>
            </a>
          </li>
        {/each}
      </ul>
    </section>
  </div>
{:else if view === 'forum' && forum}
  <nav class="f-crumbs" aria-label="Caminho">
    <a href="/f/" onclick={(e) => { e.preventDefault(); navigate('/f/'); }}>Fóruns</a>
    {#each crumbs as c (c.href)}
      <span>›</span>
      <a href={c.href} onclick={(e) => { e.preventDefault(); navigate(c.href); }}>{c.seg}</a>
    {/each}
  </nav>

  <header class="f-head">
    {#if forum.banner_url}
      <div class="f-banner" style={`background-image:url('${forum.banner_url}')`}></div>
    {/if}
    <div class="f-title-row">
      {#if forum.avatar_url}
        <img class="f-logo" src={forum.avatar_url} alt="" />
      {/if}
      <h1>{forum.name}</h1>
    </div>
    {#if forum.description}<p class="muted">{forum.description}</p>{/if}
    {#if forum.has_contact_email}
      <p class="f-badge">✉️ Instituição com e-mail vinculado — patamares: {forum.thresholds.map((t) => t.toLocaleString('pt-BR')).join(' · ')} interações</p>
    {:else}
      <p class="f-badge muted">Patamares: {forum.thresholds.map((t) => t.toLocaleString('pt-BR')).join(' · ')} interações (e-mail institucional em curadoria)</p>
    {/if}
    {#if isAdmin}
      <details class="f-admin">
        <summary>🛠️ Configurar este fórum (admin)</summary>
        <div class="f-admin-form">
          <label>
            E-mail institucional
            <input class="input" type="email" bind:value={admEmail} placeholder="ouvidoria@…" />
          </label>
          <label>
            Patamares (interações)
            <input class="input" type="text" bind:value={admThresholds} />
          </label>
          <label>
            Logo (URL da imagem)
            <input class="input" type="url" bind:value={admAvatar} placeholder="https://…/logo.png" />
          </label>
          <label>
            Capa (URL da imagem)
            <input class="input" type="url" bind:value={admBanner} placeholder="https://…/capa.jpg" />
          </label>
          <button class="btn" type="button" onclick={saveAdminInline} disabled={busy}>Salvar</button>
          {#if admMsg}<span class="note">{admMsg}</span>{/if}
        </div>
        <p class="muted small">Confirme o endereço na fonte oficial. Moderadores: em <a href="/admin/foruns">/admin/foruns</a>.</p>
      </details>
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
          <a class="f-topic" href={`/f/topico/${t.id}/${titleSlug(t.title)}`} onclick={(e) => { e.preventDefault(); navigate(`/f/topico/${t.id}/${titleSlug(t.title)}`); }}>
            <span class="f-tally" title="A favor · Contra · Ponderações">
              <span class="f-t-favor">{t.favor}</span>
              <span class="f-t-contra">{t.contra}</span>
            </span>
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
        <div class="f-md-bar">
          <label for="ft-body">Descrição</label>
          <span class="muted small">
            Markdown: **negrito** · *itálico* · `código` · [link](https://…) · - lista · # título
          </span>
          <button type="button" class="f-linklike" onclick={() => (showPreview = !showPreview)}>
            {showPreview ? '✏️ Editar' : '👁 Visualizar'}
          </button>
        </div>
        {#if showPreview}
          <div class="input f-md-preview f-topic-text">
            {#if body.trim()}
              <!-- eslint-disable-next-line svelte/no-at-html-tags — mdToHtml escapa TODO o input antes das regras -->
              {@html mdToHtml(body)}
            {:else}
              <span class="muted">Nada pra visualizar ainda…</span>
            {/if}
          </div>
        {:else}
          <textarea id="ft-body" class="input" rows="6" bind:value={body} placeholder="Contexto, problema e o que você propõe debater… (Markdown suportado)"></textarea>
        {/if}
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
    <div class="f-topic-body">
      <h1>{detail.topic.title}</h1>
      <p class="muted small">
        por {detail.topic.author_public_handle} · {fmtDate(detail.topic.created_at)} ·
        <strong>{detail.topic.interactions.toLocaleString('pt-BR')} interações</strong>
        {#if detail.topic.federated_interactions > 0}
          · 🌐 {detail.topic.federated_interactions.toLocaleString('pt-BR')} federadas (não contam pro envio)
        {/if}
      </p>
      <div class="f-topic-text">
        <!-- eslint-disable-next-line svelte/no-at-html-tags — mdToHtml escapa TODO o input antes das regras -->
        {@html mdToHtml(detail.topic.body)}
      </div>

      <!-- Placar por pontos (ADR-0019): sinal = qual lado ganha; ≥10 → encaminha ao gabinete. -->
      <div class="f-score" class:pos={detail.topic.score > 0} class:neg={detail.topic.score < 0}>
        <span class="f-score-num">{detail.topic.score > 0 ? '+' : ''}{detail.topic.score.toLocaleString('pt-BR')}</span>
        <span class="f-score-lbl">pontos</span>
        {#if detail.topic.score >= 10}
          <span class="f-score-hint ok">✓ encaminhado ao gabinete</span>
        {:else if detail.topic.score > 0}
          <span class="f-score-hint">faltam {(10 - detail.topic.score).toLocaleString('pt-BR')} pra encaminhar ao gabinete</span>
        {:else}
          <span class="f-score-hint">apoio líquido negativo — não encaminha</span>
        {/if}
      </div>

      <!-- Posições: registrar/mudar a sua + contagem de votos por lado. -->
      <div class="f-stances" role="group" aria-label="Sua posição">
        <button type="button" class="f-stance f-stance-favor" onclick={() => vote('favor')} disabled={busy}>
          A favor <strong>{detail.topic.favor.toLocaleString('pt-BR')}</strong>
        </button>
        <button type="button" class="f-stance f-stance-contra" onclick={() => vote('contra')} disabled={busy}>
          Contra <strong>{detail.topic.contra.toLocaleString('pt-BR')}</strong>
        </button>
      </div>

      <div class="f-share">
        <button type="button" class="btn" onclick={shareFediverso} disabled={shareState === 'sending'}>
          {shareState === 'sending' ? 'Publicando…' : '🌐 Compartilhar no fediverso'}
        </button>
        <button type="button" class="btn" onclick={copyLink}>🔗 Copiar link</button>
        <button type="button" class="btn" title="Denunciar este tópico à moderação" onclick={askReportTopic}>⚑ Denunciar</button>
        {#if canModerate}
          <button type="button" class="btn btn-danger" title="Remover este tópico (moderação)" onclick={removeTopic}>🗑 Remover</button>
        {/if}
        {#if shareState === 'sent'}
          <span class="muted small">
            ✅ Publicado — seus seguidores (aqui e no fediverso) receberam. <a href="/feed">Ver no feed</a>
            {#if shareApUrl}
              · Pra impulsionar de uma conta Mastodon que ainda não te segue:
              <button type="button" class="f-linklike" onclick={() => navigator.clipboard.writeText(shareApUrl)}>copiar link ActivityPub</button>
              e colar na busca de lá.
            {/if}
          </span>
        {:else if shareState === 'copied'}
          <span class="muted small">✅ Link copiado.</span>
        {:else if shareState === 'failed'}
          <span class="muted small">Não deu — tente de novo.</span>
        {/if}
      </div>

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
        <h2>Debate</h2>
        {#if detail.comments.length === 0}
          <p class="muted">Nenhum argumento ainda — seja a primeira pessoa a se posicionar.</p>
        {:else}
          <div class="f-columns">
            {#each STANCES as s (s.key)}
              <div class="f-col f-col-{s.key}">
                <h3>{s.label} <span class="muted">{columnComments(s.key).length}</span></h3>
                {#each columnComments(s.key) as c (c.id)}
                  <div class="f-arg">
                    <div class="f-topic-text">
                      <!-- eslint-disable-next-line svelte/no-at-html-tags — mdToHtml escapa TODO o input antes das regras -->
                      {@html mdToHtml(c.body)}
                    </div>
                    <span class="muted small">
                      {c.author}{#if c.author_karma != null}<span class="f-karma" title="Reputação do autor">◆ {c.author_karma.toLocaleString('pt-BR')}</span>{/if} · {fmtDate(c.created_at)}
                      · <button type="button" class="f-linklike" title="Denunciar este argumento à moderação" onclick={() => askReportComment(c)}>⚑ denunciar</button>
                      {#if canModerate}
                        · <button type="button" class="f-linklike f-linklike-danger" title="Remover este argumento (moderação)" onclick={() => removeComment(c)}>🗑 remover</button>
                      {/if}
                    </span>
                    <div class="f-arg-votes" role="group" aria-label="Votar neste argumento">
                      <button type="button" class="f-argv f-argv-favor" title="Concordo com este argumento" onclick={() => voteArg(c, 'favor')} disabled={busy}>▲ {c.favor}</button>
                      <button type="button" class="f-argv f-argv-contra" title="Discordo deste argumento" onclick={() => voteArg(c, 'contra')} disabled={busy}>▼ {c.contra}</button>
                    </div>
                  </div>
                {/each}
              </div>
            {/each}
          </div>
          {#if federatedComments.length > 0}
            <h3>🌐 Comentários do fediverso <span class="muted small">(não contam pros patamares)</span></h3>
            <ul>
              {#each federatedComments as c (c.id)}
                <li>
                  <span class="muted small">
                    🌐 {c.author} · {fmtDate(c.created_at)}
                    · <button type="button" class="f-linklike" title="Denunciar este comentário à moderação" onclick={() => askReportComment(c)}>⚑ denunciar</button>
                    {#if canModerate}
                      · <button type="button" class="f-linklike f-linklike-danger" title="Remover este comentário (moderação)" onclick={() => removeComment(c)}>🗑 remover</button>
                    {/if}
                  </span>
                  <div class="f-topic-text">
                    <!-- eslint-disable-next-line svelte/no-at-html-tags — mdToHtml escapa TODO o input antes das regras -->
                    {@html mdToHtml(c.body)}
                  </div>
                </li>
              {/each}
            </ul>
          {/if}
        {/if}

        <h3>Participar do debate</h3>
        <form onsubmit={submitComment}>
          <div class="f-stances" role="group" aria-label="Posição do seu argumento">
            <button type="button" class="f-stance f-stance-favor" class:active={commentStance === 'favor'} onclick={() => (commentStance = 'favor')}>A favor</button>
            <button type="button" class="f-stance f-stance-contra" class:active={commentStance === 'contra'} onclick={() => (commentStance = 'contra')}>Contra</button>
          </div>
          <div class="field">
            <textarea class="input" rows="3" bind:value={comment} placeholder="Seu argumento… (Markdown suportado)"></textarea>
          </div>
          <button class="btn btn-primary" type="submit" disabled={busy || !comment.trim()}>
            {busy ? 'Enviando…' : 'Publicar argumento'}
          </button>
          <p class="muted small">Seu argumento também registra sua posição no contador.</p>
        </form>
        {#if formMsg}<p class="note" role="status">{formMsg} {#if !isLogged()}<a href="/entrar">Entrar</a>{/if}</p>{/if}
      </section>
    </div>
  </article>

  <Modal
    bind:open={reportOpen}
    title="Denunciar à moderação"
    onclose={() => (reportOpen = false)}
  >
    <div class="report-form">
      <label class="rf-lbl">
        Motivo
        <select bind:value={reportCategory} class="rf-sel">
          <option value="spam">Spam</option>
          <option value="violation">Violação das regras da comunidade</option>
          <option value="other">Outro</option>
        </select>
      </label>
      <label class="rf-lbl">
        Detalhes (opcional, até 2000 caracteres)
        <textarea
          bind:value={reportReason}
          maxlength={2000}
          rows="4"
          placeholder="Descreva o que aconteceu para a moderação humana avaliar."
          class="rf-ta"
        ></textarea>
      </label>
      <p class="muted rf-hint">
        A denúncia vai pra fila de moderação da instância. A conta denunciada
        não é notificada. Uma denúncia por item por cidadão.
      </p>
    </div>
    {#snippet footer()}
      <Button variant="ghost" onclick={() => (reportOpen = false)}>Cancelar</Button>
      <Button variant="danger" onclick={submitReport} loading={reportBusy}>
        Enviar denúncia
      </Button>
    {/snippet}
  </Modal>
{/if}

<style>
  .f-head { margin-bottom: 1.25rem; }
  .report-form { display: flex; flex-direction: column; gap: 0.75rem; }
  .rf-lbl { display: flex; flex-direction: column; gap: 0.3rem; font-weight: 600; }
  .rf-sel, .rf-ta {
    font: inherit; font-weight: 400; color: inherit;
    background: transparent; border: 1px solid var(--c-border, #444);
    border-radius: 0.5rem; padding: 0.45rem 0.6rem;
  }
  .rf-hint { margin: 0; }
  .f-home { display: grid; grid-template-columns: 1fr; gap: 1.25rem; }
  @media (min-width: 860px) {
    .f-home { grid-template-columns: 20rem 1fr; align-items: start; }
  }
  .f-side details {
    border: 1px solid var(--c-border, #333);
    border-radius: 0.6rem;
    margin-bottom: 0.5rem;
    padding: 0.45rem 0.7rem;
  }
  .f-side summary { cursor: pointer; font-weight: 600; }
  .f-side nav { display: flex; flex-direction: column; gap: 0.15rem; margin-top: 0.4rem; }
  .f-side nav a {
    text-decoration: none; color: inherit; font-size: 0.92rem;
    padding: 0.22rem 0.4rem; border-radius: 0.4rem;
  }
  .f-side nav a:hover { background: rgba(127, 127, 127, 0.12); }
  .f-feed h2 { margin-top: 0; }
  .f-feed-badge {
    border: 1px solid var(--c-border, #444); border-radius: 999px;
    padding: 0 0.5rem; font-size: 0.8rem;
  }
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
  .f-topic-main { display: flex; flex-direction: column; gap: 0.15rem; }
  .f-topic-page { display: flex; gap: 1rem; }
  /* Contadores por posição (fusão debates→fóruns): verde/vermelho/cinza. */
  .f-tally { display: flex; flex-direction: column; align-items: center; gap: 0.1rem; min-width: 2.6rem; font-size: 0.82rem; font-weight: 700; }
  .f-t-favor { color: #2a9d54; }
  .f-t-favor::before { content: '▲ '; }
  .f-t-contra { color: #d64545; }
  .f-t-contra::before { content: '▼ '; }
  .f-t-ponde { opacity: 0.7; }
  .f-t-ponde::before { content: '~ '; }
  .f-score { display: flex; align-items: baseline; gap: 0.5rem; flex-wrap: wrap; margin: 0.9rem 0 0.4rem; padding: 0.6rem 0.9rem; border-radius: 10px; background: var(--surface-2, #f1f5f9); border: 1px solid var(--border-subtle, rgba(0,0,0,0.08)); }
  .f-score-num { font-size: 1.6rem; font-weight: 800; color: var(--text-3, #64748b); }
  .f-score.pos .f-score-num { color: var(--c-green-dark, #15803d); }
  .f-score.neg .f-score-num { color: #dc2626; }
  .f-score-lbl { font-size: 0.85rem; color: var(--text-3, #64748b); }
  .f-score-hint { font-size: 0.8rem; color: var(--text-3, #64748b); margin-left: auto; }
  .f-score-hint.ok { color: var(--c-green-dark, #15803d); font-weight: 600; }
  .f-karma { display: inline-block; margin-left: 0.35rem; padding: 0 0.4rem; border-radius: 6px; background: var(--accent-soft, #dcfce7); color: var(--c-green-dark, #15803d); font-weight: 700; font-size: 0.72rem; }
  .f-stances { display: flex; flex-wrap: wrap; gap: 0.5rem; margin: 0.75rem 0; }
  .f-stance {
    border: 1px solid var(--c-border, #ccc); background: none; color: inherit;
    border-radius: 999px; padding: 0.3rem 0.9rem; cursor: pointer; font-size: 0.95rem;
  }
  .f-stance strong { margin-left: 0.3rem; }
  .f-stance-favor:hover, .f-stance-favor.active { border-color: #2a9d54; color: #2a9d54; }
  .f-stance-contra:hover, .f-stance-contra.active { border-color: #d64545; color: #d64545; }
  .f-stance-ponde:hover, .f-stance-ponde.active { border-color: #8892a6; }
  .f-stance.active { font-weight: 700; }
  .f-columns { display: grid; grid-template-columns: 1fr; gap: 0.9rem; margin: 0.75rem 0; }
  @media (min-width: 760px) { .f-columns { grid-template-columns: 1fr 1fr 1fr; } }
  .f-col h3 { border-bottom: 3px solid var(--c-border, #666); padding-bottom: 0.3rem; }
  .f-col-favor h3 { border-color: #2a9d54; }
  .f-col-contra h3 { border-color: #d64545; }
  .f-col-ponderacao h3 { border-color: #8892a6; }
  .f-arg {
    border: 1px solid var(--c-border, #333); border-radius: 0.6rem;
    padding: 0.6rem 0.8rem; margin-bottom: 0.6rem;
  }
  .f-arg .f-topic-text { margin: 0 0 0.4rem; }
  .f-arg-votes { display: flex; gap: 0.4rem; margin-top: 0.45rem; }
  .f-argv {
    border: 1px solid var(--c-border, #ccc); background: none; color: inherit;
    border-radius: 999px; padding: 0.1rem 0.6rem; cursor: pointer; font-size: 0.82rem;
  }
  .f-argv-favor:hover { border-color: #2a9d54; color: #2a9d54; }
  .f-argv-contra:hover { border-color: #d64545; color: #d64545; }
  .f-argv-ponde:hover { border-color: #8892a6; }
  .f-topic-body { flex: 1; min-width: 0; }
  .f-topic-text { white-space: pre-wrap; margin: 0.75rem 0 1rem; }
  .f-share { display: flex; flex-wrap: wrap; gap: 0.5rem; align-items: center; margin: 0.75rem 0; }
  .f-receipts {
    padding: 0.6rem 0.9rem; margin: 0.75rem 0;
    background: var(--c-green-soft, #e6f7ed);
    border: 1px solid #b7e4c7; border-radius: 0.6rem; font-size: 0.92rem;
  }
  .f-comments ul { list-style: none; padding: 0; }
  .f-comments li { border-bottom: 1px solid var(--c-border, #e3e3e3); padding: 0.5rem 0; }
  .f-linklike { background: none; border: none; color: inherit; cursor: pointer; padding: 0; font-size: 0.9rem; text-decoration: underline; }
  .f-linklike-danger { color: var(--danger, #dc2626); }
  .f-banner {
    height: 9rem; border-radius: 0.7rem; margin-bottom: 0.75rem;
    background-size: cover; background-position: center;
    border: 1px solid var(--c-border, #333);
  }
  .f-title-row { display: flex; align-items: center; gap: 0.75rem; }
  .f-logo {
    width: 3.4rem; height: 3.4rem; border-radius: 0.7rem; object-fit: cover;
    border: 1px solid var(--c-border, #333); background: #fff;
  }
  .f-admin { margin: 0.5rem 0; font-size: 0.92rem; }
  .f-admin summary { cursor: pointer; }
  .f-admin-form { display: flex; flex-wrap: wrap; gap: 0.6rem; align-items: end; margin: 0.5rem 0; }
  .f-admin-form label { display: flex; flex-direction: column; gap: 0.2rem; min-width: 16rem; }
  .f-md-bar { display: flex; flex-wrap: wrap; gap: 0.6rem; align-items: baseline; justify-content: space-between; }
  .f-md-preview { min-height: 8rem; }
  .f-topic-text :global(pre) { overflow-x: auto; padding: 0.5rem; border: 1px solid var(--c-border, #444); border-radius: 0.4rem; }
  .f-topic-text :global(blockquote) { border-left: 3px solid var(--c-border, #666); margin: 0.4rem 0; padding-left: 0.7rem; opacity: 0.9; }
  .f-topic-text :global(ul), .f-topic-text :global(ol) { padding-left: 1.4rem; }
  .note { margin-top: 0.5rem; font-size: 0.92rem; }
  .small { font-size: 0.85rem; }
</style>
