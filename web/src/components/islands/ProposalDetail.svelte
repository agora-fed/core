<script lang="ts">
  // Página detalhada de proposta — CSR. Foca em VOTAR: contador grande, barra de progresso até o
  // threshold (se houver), botão Apoiar. Mostra também o político-alvo (foto + nome + link), o
  // status da SLA (relógio rolando ou silêncio público), e o corpo da demanda. Comentários
  // ficam abaixo.
  import { onMount } from 'svelte';
  import {
    getProposal,
    getMandate,
    getSlas,
    getDeliveryReceipts,
    adminHideProposal,
    adminDeleteProposal,
    DEFAULT_ORG_ID,
    apiGet,
    apiPost,
    postNote,
    type DeliveryReceiptDto,
  } from '../../lib/api';
  // (DEFAULT_ORG_ID still imported because getSlas() defaults are convenient — left for future.)
  import type { ProposalDto, MandateDto, SlaDto, SlaStatus, CommentDto } from '../../lib/types';
  import SlaClock from './SlaClock.svelte';
  import AmendmentsPanel from './AmendmentsPanel.svelte';

  let { proposalId }: { proposalId: string } = $props();

  let loading = $state(true);
  let proposal = $state<ProposalDto | null>(null);
  let mandate = $state<MandateDto | null>(null);
  let sla = $state<SlaDto | null>(null);
  let loadError = $state<string | null>(null);
  // Prova de notificação (0.29): timeline pública dos avisos ao gabinete.
  let receipts = $state<DeliveryReceiptDto[]>([]);

  // Super-admin (SOCRATES): ocultar / apagar esta proposta.
  let isAdmin = $state(false);
  let adminBusy = $state(false);
  let adminMsg = $state<string | null>(null);

  async function hideProposal() {
    if (adminBusy || !window.confirm('Ocultar esta proposta da plataforma? (reversível)')) return;
    adminBusy = true;
    const res = await adminHideProposal(proposalId, true);
    adminBusy = false;
    if (res.success) {
      window.alert('Proposta ocultada — some das listagens públicas.');
      window.location.href = '/propostas';
    } else {
      adminMsg = res.error?.message ?? 'Não foi possível ocultar.';
    }
  }

  async function deleteProposal() {
    if (adminBusy) return;
    if (!window.confirm('APAGAR DEFINITIVAMENTE esta proposta e tudo ligado a ela? Irreversível.')) return;
    if (!window.confirm('Tem certeza absoluta? Não há como desfazer.')) return;
    adminBusy = true;
    const res = await adminDeleteProposal(proposalId);
    adminBusy = false;
    if (res.success) {
      window.alert('Proposta apagada.');
      window.location.href = '/propostas';
    } else {
      adminMsg = res.error?.message ?? 'Não foi possível apagar. Tente ocultar.';
    }
  }

  // Voting state
  let count = $state(0);
  let supported = $state(false);
  let voting = $state(false);
  let voteMsg = $state<{ kind: 'error' | 'info' | 'ok'; text: string } | null>(null);

  // Debate (comments) state
  let commentsLoading = $state(true);
  let commentsError = $state<string | null>(null);
  let comments = $state<CommentDto[]>([]);
  // Root comment composer
  let newBody = $state('');
  let posting = $state(false);
  let postMsg = $state<{ kind: 'error' | 'info' | 'ok'; text: string } | null>(
    null,
  );
  // Which comment currently has its inline reply box open (id), and its state.
  let replyOpenFor = $state<string | null>(null);
  let replyBody = $state('');
  let replyPosting = $state(false);
  let replyMsg = $state<{ kind: 'error' | 'info' | 'ok'; text: string } | null>(
    null,
  );

  const COMMENT_MIN = 3;
  const COMMENT_MAX = 2000;

  // Threshold real vem do backend (P2.2). Quando a SLA existe, o limiar foi cruzado.
  let threshold = $derived(proposal?.threshold ?? 100);
  let progressPct = $derived(
    sla
      ? 100
      : threshold > 0
        ? Math.min(100, Math.round((count / threshold) * 100))
        : 0,
  );
  let thresholdCrossed = $derived(sla !== null);
  let authorLabel = $derived(
    proposal?.author_handle
      ? `@${proposal.author_handle}`
      : proposal?.author_public_handle ?? null,
  );

  // "Sou o autor?" — mostra o recibo de entrega. citizen_id local ↔
  // proposta.author_public_handle (formato u-<simple>).
  let isAuthor = $derived.by(() => {
    if (typeof window === 'undefined') return false;
    const me = readCitizenId();
    if (!me || !proposal?.author_public_handle) return false;
    const expected = `u-${me.replace(/-/g, '')}`;
    return proposal.author_public_handle === expected;
  });

  let publishing = $state(false);
  let publishMsg = $state<{ kind: 'error' | 'info' | 'ok'; text: string } | null>(null);

  async function publishToFediverse() {
    if (!proposal || publishing) return;
    if (!readCitizenId()) {
      publishMsg = { kind: 'info', text: 'Entre na sua conta para publicar.' };
      return;
    }
    publishing = true;
    publishMsg = null;
    const url = window.location.href;
    const mandateBit = mandate
      ? ` para ${mandate.display_name}${mandate.party ? ` (${mandate.party})` : ''}`
      : '';
    const content = `Nova proposta cidadã${mandateBit}: "${proposal.title}"\n\nApoie e amplie:\n${url}\n\n#DemocraciaBR`;
    const res = await postNote(content);
    publishing = false;
    publishMsg = res.success
      ? { kind: 'ok', text: 'Publicado no fediverso. Federou pra quem te segue.' }
      : { kind: 'error', text: res.error?.message ?? 'Não foi possível publicar.' };
  }

  onMount(async () => {
    try {
      isAdmin = localStorage.getItem('dsoc_is_admin') === '1';
    } catch {
      isAdmin = false;
    }
    const [pr, slr] = await Promise.all([
      getProposal(proposalId),
      getSlas(DEFAULT_ORG_ID, 500),
    ]);
    if (!pr.ok || !pr.data) {
      loading = false;
      loadError = pr.error ?? 'Proposta não encontrada.';
      return;
    }
    proposal = pr.data;
    count = proposal.support_count;
    if (slr.ok && slr.data) {
      sla = slr.data.find((s) => s.proposal_id === proposalId) ?? null;
    }
    const mr = await getMandate(proposal.mandate_id);
    if (mr.ok && mr.data) mandate = mr.data;
    loading = false;
    // Load the debate in parallel with mandate but AFTER we know the proposal exists.
    loadComments();
    // Prova de notificação — best-effort, a página funciona sem ela.
    void getDeliveryReceipts(proposalId).then((r) => {
      if (r.success && r.data) receipts = r.data;
    });
  });

  async function loadComments() {
    commentsLoading = true;
    commentsError = null;
    // Backend endpoint: GET /api/v1/comments?org_id=&proposal_id=&limit=
    // Server returns rows ordered by (created_at, id) which matches the "oldest-first" debate.
    const path =
      `/api/v1/comments?org_id=${encodeURIComponent(DEFAULT_ORG_ID)}` +
      `&proposal_id=${encodeURIComponent(proposalId)}&limit=200`;
    const res = await apiGet<CommentDto[]>(path);
    if (!res.ok || !res.data) {
      commentsError = res.error ?? 'Não foi possível carregar o debate.';
      commentsLoading = false;
      return;
    }
    comments = res.data;
    commentsLoading = false;
  }

  function readCitizenId(): string | null {
    try {
      return localStorage.getItem('dsoc_citizen');
    } catch {
      return null;
    }
  }

  // --- Debate helpers --------------------------------------------------------

  /** Public author label: display name, else `@handle`, else the opaque public
   *  handle, else a stable short-id fallback. */
  function authorLabelFor(c: CommentDto): string {
    if (c.author_display_name) return c.author_display_name;
    if (c.author_handle) return `@${c.author_handle}`;
    if (c.author_public_handle) return c.author_public_handle;
    const short = c.author_id.replace(/-/g, '').slice(0, 6);
    return `Cidadão · ${short}`;
  }

  /** "há 3h", "há 2 dias" — small, dependency-free relative time in pt-BR. */
  function relativeTime(iso: string): string {
    const now = Date.now();
    const then = new Date(iso).getTime();
    if (!Number.isFinite(then)) return '';
    const diff = Math.max(0, now - then);
    const s = Math.floor(diff / 1000);
    if (s < 60) return 'agora';
    const m = Math.floor(s / 60);
    if (m < 60) return `há ${m} min`;
    const h = Math.floor(m / 60);
    if (h < 24) return `há ${h}h`;
    const d = Math.floor(h / 24);
    if (d < 30) return d === 1 ? 'há 1 dia' : `há ${d} dias`;
    const mo = Math.floor(d / 30);
    if (mo < 12) return mo === 1 ? 'há 1 mês' : `há ${mo} meses`;
    const y = Math.floor(mo / 12);
    return y === 1 ? 'há 1 ano' : `há ${y} anos`;
  }

  /** Group comments as roots + replies keyed by their parent. Depth is capped
   *  visually at 1 (shallow thread by design). */
  let debateRoots = $derived(comments.filter((c) => !c.parent_id));
  function repliesOf(parentId: string): CommentDto[] {
    return comments.filter((c) => c.parent_id === parentId);
  }

  async function submitRoot(event: SubmitEvent) {
    event.preventDefault();
    const trimmed = newBody.trim();
    if (trimmed.length < COMMENT_MIN || trimmed.length > COMMENT_MAX || posting)
      return;
    postMsg = null;

    const citizenId = readCitizenId();
    if (!citizenId) {
      postMsg = { kind: 'info', text: 'Entre na sua conta para comentar.' };
      return;
    }

    posting = true;
    const res = await apiPost<CommentDto>('/api/v1/comments', {
      org_id: DEFAULT_ORG_ID,
      citizen_id: citizenId,
      proposal_id: proposalId,
      body: trimmed,
    });
    posting = false;

    if (res.success && res.data) {
      newBody = '';
      postMsg = { kind: 'ok', text: 'Comentário publicado.' };
      // Optimistic append — cheap, keeps the debate lively without a full refetch.
      comments = [...comments, res.data];
    } else {
      postMsg = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível enviar seu comentário.',
      };
    }
  }

  function openReply(commentId: string) {
    if (replyOpenFor === commentId) {
      // Toggle closed.
      replyOpenFor = null;
      replyBody = '';
      replyMsg = null;
      return;
    }
    replyOpenFor = commentId;
    replyBody = '';
    replyMsg = null;
  }

  async function submitReply(event: SubmitEvent, parentId: string) {
    event.preventDefault();
    const trimmed = replyBody.trim();
    if (
      trimmed.length < COMMENT_MIN ||
      trimmed.length > COMMENT_MAX ||
      replyPosting
    )
      return;
    replyMsg = null;

    const citizenId = readCitizenId();
    if (!citizenId) {
      replyMsg = { kind: 'info', text: 'Entre na sua conta para responder.' };
      return;
    }

    replyPosting = true;
    const res = await apiPost<CommentDto>('/api/v1/comments', {
      org_id: DEFAULT_ORG_ID,
      citizen_id: citizenId,
      proposal_id: proposalId,
      parent_id: parentId,
      body: trimmed,
    });
    replyPosting = false;

    if (res.success && res.data) {
      comments = [...comments, res.data];
      replyBody = '';
      replyOpenFor = null;
      replyMsg = null;
    } else {
      replyMsg = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível enviar a resposta.',
      };
    }
  }

  async function support() {
    if (voting || supported) return;
    voting = true;
    voteMsg = null;
    // Soft auth check: if the citizen never logged in (no marker in localStorage), guide them
    // before the round-trip. Identity itself comes from the HttpOnly cookie via the gateway
    // middleware — we never send citizen_id in the body anymore (ADR-0007).
    if (!readCitizenId()) {
      voteMsg = { kind: 'info', text: 'Entre na sua conta para apoiar.' };
      voting = false;
      return;
    }
    count += 1;
    supported = true;
    const res = await apiPost<ProposalDto>('/api/v1/votes', {
      proposal_id: proposalId,
    });
    if (!res.success) {
      count -= 1;
      supported = false;
      const code = res.error?.code;
      // 409 = "you already supported" — keep the optimistic state, just flip the label.
      if (code === 'conflict') {
        count += 1;
        supported = true;
        voteMsg = { kind: 'ok', text: 'Você já tinha apoiado esta proposta.' };
      } else {
        voteMsg = {
          kind: 'error',
          text: res.error?.message ?? 'Não foi possível apoiar.',
        };
      }
    } else {
      voteMsg = { kind: 'ok', text: 'Apoio registrado. Obrigado.' };
    }
    voting = false;
  }

  function share() {
    const url = window.location.href;
    const title = proposal?.title ?? 'Demanda pública na DemocraciaBR';
    if (navigator.share) {
      navigator.share({ title, url }).catch(() => {});
    } else {
      navigator.clipboard?.writeText(url);
      voteMsg = { kind: 'ok', text: 'Link copiado.' };
    }
  }
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if loadError}
  <div class="card center" role="alert">
    <h2>{loadError}</h2>
    <p class="muted"><a href="/propostas">Voltar para propostas</a></p>
  </div>
{:else if proposal}
  <article class="proposal">
    <!-- Cabeçalho com o destinatário em destaque (o "a quem isto se dirige") -->
    {#if mandate}
      <a class="target" href={`/politicos/${mandate.id}`}>
        {#if mandate.avatar_url}
          <img class="target-avatar" src={mandate.avatar_url} alt="" />
        {:else}
          <span class="target-avatar avatar-placeholder">👤</span>
        {/if}
        <div>
          <span class="target-label muted">Demanda direcionada a:</span>
          <strong class="target-name">{mandate.display_name}</strong>
          <span class="muted">
            {mandate.party}/{mandate.uf} ·
            {mandate.house === 'camara' ? 'Câmara' : mandate.house === 'senado' ? 'Senado' : ''}
          </span>
        </div>
      </a>
    {/if}

    <!-- Multi-destinatário (0537): co-gabinetes além do principal. -->
    {#if proposal.targets && proposal.targets.length > 1}
      <p class="co-targets">
        <span class="muted">Também dirigida a:</span>
        {#each proposal.targets.slice(1) as t (t.mandate_id)}
          <a href={`/politicos/${t.mandate_id}`}>{t.display_name}</a>
        {/each}
      </p>
    {/if}

    <h1>{proposal.title}</h1>

    {#if isAdmin}
      <div class="admin-bar">
        <span class="admin-tag">🛠️ Admin</span>
        <button type="button" onclick={hideProposal} disabled={adminBusy}>Ocultar</button>
        <button type="button" class="danger" onclick={deleteProposal} disabled={adminBusy}>Apagar definitivo</button>
        {#if adminMsg}<span class="admin-msg">{adminMsg}</span>{/if}
      </div>
    {/if}

    {#if isAuthor && (proposal.notified_author_at || proposal.notified_mandate_at || (proposal.targets ?? []).some((t) => t.notified_at))}
      <aside class="receipt" aria-label="Recibo de entrega">
        {#if proposal.targets && proposal.targets.some((t) => t.notified_at)}
          <!-- Recibo por gabinete (0537). -->
          {#each proposal.targets.filter((t) => t.notified_at) as t (t.mandate_id)}
            <p>
              ✉️ E-mail entregue ao gabinete
              <strong>{t.display_name}</strong>
              em <time datetime={t.notified_at}>
                {new Date(t.notified_at ?? '').toLocaleString('pt-BR', {
                  dateStyle: 'short',
                  timeStyle: 'short',
                })}
              </time>
            </p>
          {/each}
        {:else if proposal.notified_mandate_at}
          <p>
            ✉️ E-mail entregue ao gabinete
            {#if mandate}
              <strong>{mandate.display_name}</strong>
            {/if}
            em <time datetime={proposal.notified_mandate_at}>
              {new Date(proposal.notified_mandate_at).toLocaleString('pt-BR', {
                dateStyle: 'short',
                timeStyle: 'short',
              })}
            </time>
          </p>
        {/if}
        {#if proposal.notified_author_at}
          <p class="muted small">
            (Você também recebeu uma cópia por e-mail em
            {new Date(proposal.notified_author_at).toLocaleTimeString('pt-BR', {
              hour: '2-digit',
              minute: '2-digit',
            })}.)
          </p>
        {/if}
      </aside>
    {/if}

    {#if authorLabel}
      <p class="author">
        {#if proposal.author_avatar_url}
          <img class="author-avatar" src={proposal.author_avatar_url} alt="" />
        {:else}
          <span class="author-avatar avatar-placeholder">👤</span>
        {/if}
        <span class="muted">Proposto por</span>
        <strong>{authorLabel}</strong>
      </p>
    {/if}

    {#if thresholdCrossed}
      <aside class="threshold-banner" role="status">
        <div class="tb-icon">🚨</div>
        <div class="tb-copy">
          <strong>Limiar atingido — o relógio começou a correr</strong>
          <p>
            O gabinete de {mandate?.display_name ?? 'do mandato'} foi
            notificado. Agora é responder no prazo ou o silêncio vira
            registro público.
          </p>
        </div>
        <div class="tb-cta">
          {#if isAuthor}
            <button
              class="btn btn-primary"
              type="button"
              onclick={publishToFediverse}
              disabled={publishing}
            >
              🌐 Amplifique no fediverso
            </button>
          {:else}
            <button class="btn btn-primary" type="button" onclick={share}>
              Compartilhar
            </button>
          {/if}
        </div>
      </aside>
    {/if}

    <!-- Bloco GRANDE de votação — o coração da página -->
    <section class="vote-block" class:crossed={thresholdCrossed}>
      <div class="vote-stats">
        <p class="count">
          <strong>{count.toLocaleString('pt-BR')}</strong>
          {count === 1 ? 'pessoa apoia' : 'pessoas apoiam'}
        </p>
        {#if thresholdCrossed}
          <p class="threshold-met">
            ✓ Limiar atingido — relógio de resposta correndo
          </p>
        {:else}
          <p class="threshold muted">
            Quando atingir <strong>{threshold.toLocaleString('pt-BR')}</strong> apoios,
            começa o prazo de resposta.
          </p>
        {/if}
      </div>
      <div class="progress" aria-label={`${progressPct}%`}>
        <div class="progress-fill" style={`width:${progressPct}%`}></div>
      </div>
      <div class="actions">
        <button
          class="btn btn-primary btn-lg"
          type="button"
          onclick={support}
          disabled={voting || supported}
          aria-pressed={supported}
        >
          {#if supported}
            ✓ Você apoia
          {:else}
            Apoiar esta demanda
          {/if}
        </button>
        <button class="btn btn-ghost" type="button" onclick={share}>
          Compartilhar
        </button>
        {#if isAuthor}
          <button
            class="btn btn-ghost"
            type="button"
            onclick={publishToFediverse}
            disabled={publishing}
            title="Publica uma nota na sua timeline federada com o link desta proposta"
          >
            {publishing ? 'Publicando…' : 'Publicar no fediverso'}
          </button>
        {/if}
      </div>
      {#if publishMsg}
        <p class={`vote-msg ${publishMsg.kind}`} role="status">
          {publishMsg.text}
        </p>
      {/if}
      {#if voteMsg}
        <p class={`vote-msg ${voteMsg.kind}`} role="status">
          {voteMsg.text}
          {#if voteMsg.kind === 'info'}<a href="/entrar">Entrar</a>{/if}
        </p>
      {/if}
    </section>

    <!-- SLA clock quando o limiar já foi cruzado -->
    {#if sla}
      <section class="sla-block">
        <h2>⏰ Prazo de resposta</h2>
        <SlaClock
          dueAt={sla.due_at}
          status={sla.status as SlaStatus}
        />
        <p class="muted">
          Se o prazo expirar sem resposta, fica como <strong>silêncio público</strong>
          permanente no placar do(a) parlamentar.
        </p>
      </section>
    {/if}

    <!-- Prova de notificação (0.29): a timeline pública dos avisos ao
         gabinete — cada tentativa com recibo hash-encadeado verificável. -->
    {#if receipts.length > 0}
      <section class="receipts">
        <h2>📨 Avisos ao gabinete — com recibo</h2>
        <p class="muted">
          Cada aviso enviado fica lavrado num recibo encadeado por hash:
          adulterar qualquer um quebra a cadeia. O silêncio, se houver, é
          provado — não alegado.
        </p>
        <ol class="receipt-list">
          {#each receipts as r (r.attempt)}
            <li class="receipt">
              <span class="receipt-when">
                {r.attempt}º aviso —
                {new Date(r.sent_at).toLocaleDateString('pt-BR', {
                  day: '2-digit', month: '2-digit', year: 'numeric',
                })}
              </span>
              <span class="receipt-outcome" class:ok={r.outcome === 'accepted'}>
                {r.outcome === 'accepted' ? 'entregue' : r.outcome}
              </span>
              <code class="receipt-hash" title={`sha256 completo: ${r.hash}`}>
                …{r.hash.slice(-16)}
              </code>
            </li>
          {/each}
        </ol>
      </section>
    {/if}

    <!-- Corpo da proposta -->
    <section class="body">
      <h2>Sobre a demanda</h2>
      <p class="body-text">{proposal.body}</p>
    </section>

    <!-- Emendas (Decidim gap parity): variantes propostas por outros cidadãos -->
    <section class="amendments">
      <AmendmentsPanel
        proposalId={proposalId}
        proposalBody={proposal.body}
      />
    </section>

    <!-- Debate: thread rasa de comentários (roots + 1 nível de resposta) -->
    <section class="comments">
      <h2>Debate</h2>

      {#if commentsLoading}
        <p class="muted">Carregando debate…</p>
      {:else if commentsError}
        <p class="note error" role="alert">
          {commentsError}
          <button class="link-btn" type="button" onclick={loadComments}>Tentar de novo</button>
        </p>
      {:else}
        {#if debateRoots.length === 0}
          <p class="muted">
            Nenhum comentário ainda. Seja a primeira pessoa a debater essa proposta.
          </p>
        {:else}
          <ul class="comment-list">
            {#each debateRoots as c (c.id)}
              <li class="comment">
                <p class="comment-meta">
                  <strong>{authorLabelFor(c)}</strong>
                  <span class="muted">{relativeTime(c.created_at)}</span>
                </p>
                <p class="comment-body">{c.body}</p>
                <button class="link-btn" type="button" onclick={() => openReply(c.id)}>
                  {replyOpenFor === c.id ? 'Cancelar' : 'Responder'}
                </button>

                {#each repliesOf(c.id) as r (r.id)}
                  <div class="comment reply">
                    <p class="comment-meta">
                      <strong>{authorLabelFor(r)}</strong>
                      <span class="muted">{relativeTime(r.created_at)}</span>
                    </p>
                    <p class="comment-body">{r.body}</p>
                  </div>
                {/each}

                {#if replyOpenFor === c.id}
                  <form
                    class="reply-form"
                    onsubmit={(e) => submitReply(e, c.id)}
                    novalidate
                  >
                    <textarea
                      class="input"
                      rows="3"
                      maxlength={COMMENT_MAX}
                      bind:value={replyBody}
                      placeholder="Sua resposta…"
                    ></textarea>
                    <button
                      class="btn btn-primary"
                      type="submit"
                      disabled={replyPosting || replyBody.trim().length < COMMENT_MIN}
                    >
                      {replyPosting ? 'Enviando…' : 'Publicar resposta'}
                    </button>
                    {#if replyMsg}
                      <p class={`note ${replyMsg.kind}`} role="status">
                        {replyMsg.text}
                        {#if replyMsg.kind === 'info'}<a href="/entrar">Entrar</a>{/if}
                      </p>
                    {/if}
                  </form>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}

        <form class="comment-form" onsubmit={submitRoot} novalidate>
          <label for="new-comment">Seu comentário</label>
          <textarea
            id="new-comment"
            class="input"
            rows="4"
            maxlength={COMMENT_MAX}
            bind:value={newBody}
            placeholder="Contribua com argumentos, dados ou uma sugestão construtiva…"
          ></textarea>
          <button
            class="btn btn-primary"
            type="submit"
            disabled={posting || newBody.trim().length < COMMENT_MIN}
          >
            {posting ? 'Enviando…' : 'Publicar comentário'}
          </button>
          {#if postMsg}
            <p class={`note ${postMsg.kind}`} role="status">
              {postMsg.text}
              {#if postMsg.kind === 'info'}<a href="/entrar">Entrar</a>{/if}
            </p>
          {/if}
        </form>
      {/if}
    </section>
  </article>
{/if}

<style>
  .proposal {
    max-width: 44rem;
    margin: 0 auto;
  }
  .target {
    display: flex;
    gap: 0.85rem;
    align-items: center;
    padding: 0.85rem 1rem;
    background: var(--c-bg);
    border-radius: 12px;
    text-decoration: none;
    color: inherit;
    margin-bottom: 1.5rem;
    border: 1px solid var(--c-border);
  }
  .target:hover {
    background: var(--c-paper);
  }
  .target-avatar {
    width: 52px;
    height: 52px;
    border-radius: 50%;
    object-fit: cover;
    background: var(--c-paper);
    flex-shrink: 0;
  }
  .avatar-placeholder {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 1.4rem;
  }
  .target-label {
    display: block;
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .target-name {
    display: block;
    font-size: 1.05rem;
  }
  .co-targets {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem 0.6rem;
    margin: 0.35rem 0 0.9rem;
    font-size: var(--fs-sm, 0.9rem);
  }
  .co-targets a {
    text-decoration: underline;
  }
  h1 {
    font-size: 1.7rem;
    margin: 0 0 0.6rem;
    line-height: 1.25;
  }
  .receipt {
    display: grid;
    gap: 4px;
    padding: 10px 14px;
    margin: 12px 0 16px;
    background: var(--c-green-soft, #e6f7ed);
    border: 1px solid #b7e4c7;
    border-radius: 10px;
    font-size: var(--fs-sm, 0.9rem);
    color: var(--c-green-dark, #115c2d);
  }
  .receipt p {
    margin: 0;
  }
  .receipt .small {
    font-size: var(--fs-xs, 0.8rem);
    color: var(--c-text-muted);
  }
  .author {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0 0 1.5rem;
    font-size: 0.95rem;
  }
  .author-avatar {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    object-fit: cover;
    background: var(--c-bg);
  }
  .author .muted {
    font-size: 0.88rem;
  }
  .threshold-banner {
    display: grid;
    grid-template-columns: auto 1fr auto;
    gap: 1rem;
    align-items: center;
    padding: 1rem 1.25rem;
    margin: 0 0 1.5rem;
    background: linear-gradient(135deg, #fff7e6 0%, #ffe4b5 100%);
    border: 1.5px solid #ffb020;
    border-radius: 12px;
  }
  @media (max-width: 640px) {
    .threshold-banner {
      grid-template-columns: 1fr;
      text-align: center;
    }
  }
  .tb-icon {
    font-size: 2rem;
    line-height: 1;
  }
  .tb-copy strong {
    display: block;
    color: #7a4a00;
    font-size: 1rem;
    margin-bottom: 4px;
  }
  .tb-copy p {
    margin: 0;
    font-size: 0.9rem;
    color: #5a3800;
    line-height: 1.5;
  }
  .vote-block {
    background: var(--c-paper);
    border: 2px solid var(--c-border);
    border-radius: 14px;
    padding: 1.5rem;
    margin-bottom: 2rem;
  }
  .vote-block.crossed {
    border-color: var(--c-green-dark);
    background: var(--c-green-soft);
  }
  .vote-stats {
    text-align: center;
    margin-bottom: 1rem;
  }
  .count {
    margin: 0;
    font-size: 1rem;
  }
  .count strong {
    font-size: 2.4rem;
    color: var(--c-navy);
    display: block;
    line-height: 1;
    font-variant-numeric: tabular-nums;
  }
  .threshold {
    margin: 0.5rem 0 0;
    font-size: 0.92rem;
  }
  .threshold-met {
    margin: 0.5rem 0 0;
    color: var(--c-green-dark);
    font-weight: 600;
  }
  .progress {
    height: 10px;
    background: var(--c-bg);
    border-radius: 999px;
    overflow: hidden;
    margin-bottom: 1.25rem;
  }
  .progress-fill {
    display: block;
    height: 100%;
    background: var(--c-green);
    transition: width 240ms ease;
  }
  .vote-block.crossed .progress-fill {
    background: var(--c-green-dark);
  }
  .actions {
    display: flex;
    gap: 0.6rem;
    justify-content: center;
    flex-wrap: wrap;
  }
  .vote-msg {
    margin: 0.85rem 0 0;
    text-align: center;
    font-size: 0.92rem;
  }
  .vote-msg.error { color: var(--c-ignored); }
  .vote-msg.ok    { color: var(--c-green-dark); font-weight: 600; }
  .vote-msg.info  { color: var(--c-text-muted); }
  .sla-block {
    border: 1px solid var(--c-border);
    border-radius: 12px;
    padding: 1.25rem;
    margin-bottom: 2rem;
  }
  .receipts {
    border: 1px solid var(--c-border);
    border-radius: 12px;
    padding: 1.25rem;
    margin-bottom: 2rem;
  }
  .receipt-list {
    list-style: none;
    padding: 0;
    margin: 0.75rem 0 0;
    display: grid;
    gap: 0.4rem;
  }
  .receipt {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0.6rem;
    font-size: 0.92rem;
  }
  .receipt-outcome {
    font-weight: 600;
  }
  .receipt-outcome.ok {
    color: var(--c-green-dark);
  }
  .receipt-hash {
    font-size: 0.8rem;
    opacity: 0.7;
    overflow-wrap: anywhere;
  }
  .sla-block h2 {
    margin: 0 0 0.8rem;
    font-size: 1.05rem;
  }
  .comment-list {
    list-style: none;
    padding: 0;
    margin: 0 0 1.5rem;
    display: grid;
    gap: 1rem;
  }
  .comment {
    border: 1px solid var(--c-border);
    border-radius: 10px;
    padding: 0.9rem 1.1rem;
  }
  .comment.reply {
    margin: 0.75rem 0 0 1.5rem;
    background: var(--c-bg);
    border-radius: 8px;
  }
  .comment-meta {
    margin: 0 0 0.35rem;
    font-size: 0.88rem;
    display: flex;
    gap: 0.6rem;
    align-items: baseline;
  }
  .comment-body {
    margin: 0;
    white-space: pre-wrap;
  }
  .link-btn {
    background: none;
    border: none;
    padding: 0;
    margin-top: 0.5rem;
    color: var(--c-navy);
    font-size: 0.88rem;
    cursor: pointer;
    text-decoration: underline;
  }
  .reply-form,
  .comment-form {
    margin-top: 0.9rem;
    display: grid;
    gap: 0.5rem;
    justify-items: start;
  }
  .comment-form label {
    font-weight: 600;
    font-size: 0.95rem;
  }
  .comments textarea.input {
    resize: vertical;
    width: 100%;
    justify-self: stretch;
  }
  .note {
    margin: 0;
    font-size: 0.92rem;
  }
  .note.error { color: var(--c-ignored); }
  .note.ok { color: var(--c-green-dark); }
  .note.info { color: var(--c-text-muted); }

  .body h2, .comments h2 {
    margin: 0 0 0.8rem;
    font-size: 1.05rem;
  }
  .body-text {
    white-space: pre-wrap;
    line-height: 1.6;
    margin: 0 0 2.5rem;
  }
  .center {
    text-align: center;
    padding: 2.5rem 1.5rem;
  }
  /* Barra super-admin (SOCRATES) */
  .admin-bar {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex-wrap: wrap;
    margin: 0.75rem 0 1rem;
    padding: 0.5rem 0.75rem;
    border: 1px dashed var(--border-subtle, #ccc);
    border-radius: 8px;
  }
  .admin-tag { font-weight: 700; font-size: 0.85rem; color: var(--text-2, inherit); }
  .admin-bar button {
    padding: 0.35rem 0.8rem;
    border-radius: 7px;
    border: 1px solid var(--border-subtle, #ccc);
    background: var(--surface-1, #fff);
    color: var(--text-1, inherit);
    font-weight: 600;
    cursor: pointer;
  }
  .admin-bar button.danger { color: #dc2626; border-color: #dc2626; }
  .admin-bar button:disabled { opacity: 0.5; cursor: default; }
  .admin-msg { font-size: 0.85rem; color: #dc2626; }
</style>
